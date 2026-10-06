//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! r20/r21 shell-oracle replay: drives the ported interactive shell through
//! the same seams the node harness recorded
//! (`tests/fixtures/interactive_r20_oracle/lower_oracle.json`, 134 scenarios over
//! verbatim upstream bodies) and compares the collaborator log byte-for-byte.
//!
//! Recording vocabulary (S1/S1a): containers/components/indicators through the
//! typed [`ShellView`] methods, everything else through the `emit` tuple pump.
//! A `{"__describe": kind}` argument marker renders the TUI handle describe
//! (`{"kind": …}`) the JS stubs produce for `this.ui`. Harness conventions
//! reproduced here: plain-object children record as `"[object Object]"`,
//! `dispose` calls carry no argument, top-level array arguments spread into
//! one log entry per element, and a settings `Value::Null` renders as
//! `"undefined"` where the JS driver passed `undefined`.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use super::interactive_mode::{
    AuthProviderOption, BashOutcome, CacheMiss, CacheWaste, ComponentKind, ComponentRef,
    ContainerId, EditorBorder, ExtensionCommandInfo, ExtensionShortcut, FocusTarget, ForkOutcome,
    ForkableUserMessage, HostError, InteractiveModeOptions, LoadedResource, LoginCallbacks,
    LoginError, ModelRef, NavigateOutcome, NewSessionOutcome, RefreshResult, ResourceDiagnostic,
    ResourceGroupRead, SessionStats, ShellClock, ShellEditor, ShellExtensionSurface, ShellHost,
    ShellModelRuntime, ShellPlatform, ShellResources, ShellSession, ShellSessionManager,
    ShellSettings, ShellShortcutSurface, ShellView, UsageCostRow, UserBashOutcome,
};
use super::shell::{
    ChangelogSource, CommandSink, ExtensionDialog, InteractiveMode, PackageUpdatesSource,
    SelectorToken, ShellCommand, ShellIo,
};
use super::theme::{load_builtin_theme, ColorMode};
use crate::agent_core::types::{AgentMessage, ThinkingLevel};
use crate::coding_agent::agent_session::{AgentSessionError, CycleDirection, ModelCycleResult};
use crate::coding_agent::core::settings_manager::QuietStartup;
use crate::coding_agent::extensions::types::StreamingDelivery;
use crate::coding_agent::package_manager::{
    CommandError, CommandRunner, DefaultPackageManager, PackageManagerOptions, PackageSourceEntry,
    PackageUpdate, SettingsData, SettingsManagerHandle,
};
use crate::coding_agent::session_manager::SessionEntry;

type Log = Arc<Mutex<Vec<Value>>>;

fn rec(log: &Log, parts: Value) {
    log.lock().expect("log").push(parts);
}

/// Renders an argument: the `__describe` marker becomes the recorded handle
/// shape the JS stubs produce.
fn render_arg(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(render_arg).collect()),
        Value::Object(map) => {
            if let Some(kind) = map.get("__describe").and_then(Value::as_str) {
                match kind {
                    "ui" => json!({ "kind": "ui" }),
                    "editorContainer" => json!({ "container": "editorContainer", "children": [] }),
                    // The plain editor object describe (fixture at defaults).
                    "editor" => editor_describe(false),
                    other => json!({ "kind": other }),
                }
            } else {
                Value::Object(
                    map.iter()
                        .map(|(k, v)| (k.clone(), render_arg(v)))
                        .collect(),
                )
            }
        }
        other => other.clone(),
    }
}

/// The default (`fakeEditor`) object describe from the harness.
fn editor_describe(custom: bool) -> Value {
    let escape = if custom { "function" } else { "undefined" };
    let ctrl_d = if custom { "function" } else { "undefined" };
    let paste = if custom { "function" } else { "undefined" };
    let extension_shortcut = if custom { "function" } else { "undefined" };
    json!({
        "_editorName": "<state>", "_state": "<state>",
        "onEscape": escape, "onCtrlD": ctrl_d, "onSubmit": "undefined",
        "onChange": "undefined", "onPasteImage": paste,
        "onExtensionShortcut": extension_shortcut, "embedWorkingStatus": true,
        "actionHandlers": {},
        "getText": "function", "getExpandedText": "function", "setText": "function",
        "addToHistory": "function", "insertTextAtCursor": "function",
        "handleInput": "function", "setWorkingStatusIndicator": "function",
        "setAutocompleteProvider": "function", "setPaddingX": "function",
        "getPaddingX": "function", "setAutocompleteMaxVisible": "function",
        "getAutocompleteMaxVisible": "function", "onAction": "function",
    })
}

// ---------------------------------------------------------------------------
// Recording collaborators
// ---------------------------------------------------------------------------

struct RecView {
    log: Log,
    ids: AtomicU64,
    texts: Mutex<BTreeMap<u64, String>>,
    chat_adds: AtomicU64,
    overlays: AtomicBool,
    /// Whether the focused editor is the custom editor (driver-flipped).
    editor_custom: AtomicBool,
    /// The renderer mode (`ui.mode`).
    mode: Mutex<String>,
    /// Children recorded into the detached reload box (`container`).
    box_children: Mutex<Vec<Value>>,
}

impl RecView {
    fn new(log: Log) -> Self {
        Self {
            log,
            ids: AtomicU64::new(1),
            texts: Mutex::new(BTreeMap::new()),
            chat_adds: AtomicU64::new(0),
            overlays: AtomicBool::new(false),
            editor_custom: AtomicBool::new(false),
            mode: Mutex::new("regular".to_string()),
            box_children: Mutex::new(Vec::new()),
        }
    }
    fn id(&self) -> u64 {
        self.ids.fetch_add(1, Ordering::SeqCst)
    }
}

fn container_name(container: ContainerId) -> &'static str {
    container.as_str()
}

/// The detached reload box renders as the plain-object describe the harness
/// produces for a `Container` instance.
fn box_describe(children: &[Value]) -> Value {
    json!({ "container": "container", "children": children })
}

/// The plain-object describe of a box child (`describeArg` walks the instance
/// fields instead of the component describe).
fn plain_box_child(child: &Value) -> Value {
    let mut out = serde_json::Map::new();
    if let Some(map) = child.as_object() {
        for (key, value) in map {
            if key == "kind" {
                if value.as_str() == Some("Spacer") {
                    out.insert("height".to_string(), json!(1));
                }
                continue;
            }
            out.insert(key.clone(), value.clone());
        }
    }
    Value::Object(out)
}

impl ShellView for RecView {
    fn request_render(&self, force: Option<bool>) {
        rec(
            &self.log,
            match force {
                Some(force) => json!(["ui.requestRender", force]),
                None => json!(["ui.requestRender"]),
            },
        );
    }
    fn invalidate(&self) {
        rec(&self.log, json!(["ui.invalidate"]));
    }
    fn render_now(&self) {
        rec(&self.log, json!(["ui.renderNow"]));
    }
    fn start(&self) {
        rec(&self.log, json!(["ui.start"]));
    }
    fn stop(&self, preserve_screen: bool) {
        rec(
            &self.log,
            json!(["ui.stop", { "preserveScreen": preserve_screen }]),
        );
    }
    fn set_clear_on_shrink(&self, enabled: bool) {
        rec(&self.log, json!(["ui.setClearOnShrink", enabled]));
    }
    fn set_show_hardware_cursor(&self, enabled: bool) {
        rec(&self.log, json!(["ui.setShowHardwareCursor", enabled]));
    }
    fn terminal_set_progress(&self, enabled: bool) {
        rec(&self.log, json!(["terminal.setProgress", enabled]));
    }
    fn terminal_set_title(&self, title: &str) {
        rec(&self.log, json!(["terminal.setTitle", title]));
    }
    fn drain_input(&self, ms: u64) {
        rec(&self.log, json!(["terminal.drainInput", ms]));
    }
    fn set_focus(&self, target: FocusTarget) {
        let described = match target {
            FocusTarget::Editor => editor_describe(self.editor_custom.load(Ordering::SeqCst)),
            FocusTarget::Component(component) => match component.kind.as_str() {
                // `selector.getSettingsList()`
                "SettingsSelectorList" => json!({ "updateValue": "function" }),
                // `selector.getMessageList()`
                "MessageList" => json!({ "kind": "MessageList" }),
                // The detached reload box.
                "reloadBox" => box_describe(&self.box_children.lock().expect("box")),
                other => json!({ "kind": other }),
            },
            FocusTarget::None => Value::Null,
        };
        rec(&self.log, json!(["ui.setFocus", described]));
    }
    fn show_overlay(&self, component: &ComponentRef, _options: Option<Value>) {
        rec(
            &self.log,
            json!(["ui.showOverlay", { "kind": component.kind }, "undefined"]),
        );
    }
    fn hide_overlay(&self) {
        rec(&self.log, json!(["ui.hideOverlay"]));
    }
    fn add_input_listener(&self) -> u64 {
        rec(&self.log, json!(["ui.addInputListener", "function"]));
        self.id()
    }
    fn remove_input_listener(&self, _id: u64) {
        rec(&self.log, json!(["ui.removeInputListener"]));
    }
    fn container_version(&self, container: ContainerId) -> u64 {
        match container {
            ContainerId::Chat => self.chat_adds.load(Ordering::SeqCst),
            _ => 0,
        }
    }
    fn container_clear(&self, container: ContainerId) {
        rec(
            &self.log,
            json!(["Container.clear", container_name(container)]),
        );
    }
    fn container_add_spacer(&self, container: ContainerId) -> u64 {
        if container == ContainerId::Chat {
            self.chat_adds.fetch_add(1, Ordering::SeqCst);
        }
        rec(
            &self.log,
            json!([
                "Container.addChild",
                container_name(container),
                { "kind": "Spacer" }
            ]),
        );
        self.id()
    }
    fn container_add_text(
        &self,
        container: ContainerId,
        text: &str,
        pad_x: i64,
        pad_y: i64,
        truncated: bool,
    ) -> u64 {
        if container == ContainerId::Chat {
            self.chat_adds.fetch_add(1, Ordering::SeqCst);
        }
        let id = self.id();
        rec(
            &self.log,
            json!([
                "Container.addChild",
                container_name(container),
                {
                    "kind": if truncated { "TruncatedText" } else { "Text" },
                    "text": text,
                    "paddingX": pad_x,
                    "paddingY": pad_y,
                }
            ]),
        );
        self.texts
            .lock()
            .expect("texts")
            .insert(id, text.to_string());
        id
    }
    fn container_add_expandable_text(
        &self,
        container: ContainerId,
        collapsed: &str,
        expanded: &str,
        initially_expanded: bool,
        pad_x: i64,
        pad_y: i64,
    ) -> u64 {
        // The lower drive describes an ExpandableText as its current text row.
        let text = if initially_expanded {
            expanded
        } else {
            collapsed
        };
        self.container_add_text(container, text, pad_x, pad_y, false)
    }
    fn container_set_text(&self, _container: ContainerId, id: u64, text: &str) {
        let old = self
            .texts
            .lock()
            .expect("texts")
            .get(&id)
            .cloned()
            .unwrap_or_default();
        rec(&self.log, json!(["Text.setText", old, text]));
        // The harness Text keeps its mutated content for later coalescing.
        self.texts
            .lock()
            .expect("texts")
            .insert(id, text.to_string());
    }
    fn container_add_component(&self, container: ContainerId, component: &ComponentRef) {
        // Plain-object children (the editor / footer / detached box) record as
        // their JS `String()` form; component stubs record their describe.
        let described: Value = match component.kind.as_str() {
            "editor" | "customEditor" | "footer" | "reloadBox" => json!("[object Object]"),
            other => json!({ "kind": other }),
        };
        rec(
            &self.log,
            json!(["Container.addChild", container_name(container), described]),
        );
    }
    fn container_remove_component(&self, container: ContainerId, component: &ComponentRef) {
        rec(
            &self.log,
            json!([
                "Container.removeChild",
                container_name(container),
                { "kind": component.kind }
            ]),
        );
    }
    fn container_replace_child(
        &self,
        container: ContainerId,
        index: usize,
        component: &ComponentRef,
    ) {
        rec(
            &self.log,
            json!([
                "Container.replaceChild",
                container_name(container),
                index,
                { "kind": component.kind }
            ]),
        );
    }
    fn container_replace_child_unrecorded(
        &self,
        _container: ContainerId,
        _index: usize,
        _component: &ComponentRef,
    ) {
    }
    fn container_children_len(&self, _container: ContainerId) -> usize {
        0
    }
    fn new_component(&self, kind: ComponentKind, args: Value) -> ComponentRef {
        let mut tuple = vec![Value::String(format!("new {}", kind.as_str()))];
        if let Some(items) = args.as_array() {
            tuple.extend(items.iter().map(render_arg));
        }
        rec(&self.log, Value::Array(tuple));
        ComponentRef {
            kind: kind.as_str().to_string(),
            id: self.id(),
        }
    }
    fn update_component(&self, component: &ComponentRef, op: &str, args: Value) {
        // External dialog stubs (id 0) are silent, like the scenario-provided
        // plain objects; `dispose()` takes no argument; array arguments spread
        // one entry each.
        if component.id == 0 {
            return;
        }
        let mut tuple = vec![Value::String(format!("{}.{}", component.kind, op))];
        if op != "dispose" && !args.is_null() {
            match args {
                Value::Array(items) => tuple.extend(items.iter().map(render_arg)),
                other => tuple.push(render_arg(&other)),
            }
        }
        rec(&self.log, Value::Array(tuple));
    }
    fn emit(&self, tuple: Value) {
        // The detached reload box is a plain container instance; capture its
        // children (as the plain-object describes the focus describe renders)
        // so the focus describe can render them.
        if tuple.get(0).and_then(Value::as_str) == Some("Container.addChild")
            && tuple.get(1).and_then(Value::as_str) == Some("container")
        {
            if let Some(child) = tuple.get(2) {
                self.box_children
                    .lock()
                    .expect("box")
                    .push(plain_box_child(child));
            }
        }
        rec(&self.log, tuple);
    }
    fn get_clear_on_shrink(&self) -> bool {
        false
    }
    fn idle_status_component(&self) -> ComponentRef {
        ComponentRef {
            kind: "IdleStatus".to_string(),
            id: 0,
        }
    }
    fn has_overlay_entries(&self) -> bool {
        self.overlays.load(Ordering::SeqCst)
    }
    fn renderer_mode(&self) -> String {
        self.mode.lock().expect("mode").clone()
    }
    fn renderer_children(&self) -> Vec<ComponentRef> {
        Vec::new()
    }
    fn renderer_focused_component(&self) -> Option<ComponentRef> {
        None
    }
    fn renderer_terminal_id(&self) -> u64 {
        1
    }
    fn renderer_show_hardware_cursor(&self) -> bool {
        false
    }
    fn renderer_capture_render_state(&self) {}
    fn renderer_create(&self, mode: &str, _terminal: u64) -> u64 {
        *self.mode.lock().expect("mode") = mode.to_string();
        rec(
            &self.log,
            json!(["createInteractiveTui", {
                "tuiMode": mode,
                "showHardwareCursor": false,
                "logDirectory": "/home/u/.pi/agent",
                "terminal": {
                    "columns": 80,
                    "rows": 24,
                    "setProgress": "function",
                    "setTitle": "function",
                    "drainInput": "function",
                },
                "onRightClickPaste": "undefined",
                "fullscreenCopyOnSelect": false,
            }]),
        );
        self.id()
    }
    fn renderer_become(&self, _id: u64) {}
    fn get_copy_on_select(&self) -> bool {
        false
    }
    fn has_active_selection(&self) -> bool {
        false
    }
    fn debug_render(&self) -> (usize, usize, Vec<String>) {
        (80, 24, vec!["line-one".to_string(), "line-two".to_string()])
    }
    fn container_components(&self, _container: ContainerId) -> Vec<ComponentRef> {
        Vec::new()
    }
    fn container_insert_at(&self, container: ContainerId, index: usize, component: &ComponentRef) {
        rec(
            &self.log,
            json!([
                "Container.insertChild",
                container_name(container),
                index,
                { "kind": component.kind }
            ]),
        );
    }
    fn container_add_border(&self, container: ContainerId, _color_tag: Option<&str>) -> u64 {
        if container == ContainerId::Chat {
            self.chat_adds.fetch_add(1, Ordering::SeqCst);
        }
        rec(
            &self.log,
            json!([
                "Container.addChild",
                container_name(container),
                { "kind": "DynamicBorder", "colorTag": "default" }
            ]),
        );
        self.id()
    }
    fn container_add_markdown(
        &self,
        container: ContainerId,
        text: &str,
        pad_x: i64,
        _pad_y: i64,
        _theme: &Value,
    ) -> u64 {
        if container == ContainerId::Chat {
            self.chat_adds.fetch_add(1, Ordering::SeqCst);
        }
        rec(
            &self.log,
            json!([
                "Container.addChild",
                container_name(container),
                { "kind": "Markdown", "text": text, "paddingX": pad_x }
            ]),
        );
        self.id()
    }
    fn container_remove_at(&self, _container: ContainerId, _component: &ComponentRef) {}
    fn session_identity(&self) -> u64 {
        1
    }
    fn user_input_resolved(&self, _slot: u64, _text: &str) {}
    fn renderer_stop_preserving_screen(&self) {
        rec(&self.log, json!(["ui.stop", { "preserveScreen": true }]));
    }
    fn renderer_set_focus_none(&self) {
        rec(&self.log, json!(["ui.setFocus", "undefined"]));
    }
    fn renderer_clear(&self) {
        rec(&self.log, json!(["ui.clear"]));
    }
    fn renderer_set_layout_root_none(&self) {
        rec(&self.log, json!(["ui.setLayoutRoot", "undefined"]));
    }
    fn renderer_invalidate(&self) {
        rec(&self.log, json!(["ui.invalidate"]));
    }
    fn renderer_start(&self) {
        rec(&self.log, json!(["ui.start"]));
    }
    fn renderer_add_child(&self, container: ContainerId) {
        rec(
            &self.log,
            json!(["ui.addChild", { "container": container_name(container) }]),
        );
    }
    fn header_unshift(&self, component: &ComponentRef) {
        rec(
            &self.log,
            json!([
                "Container.unshift",
                "header",
                { "kind": component.kind }
            ]),
        );
    }
    fn renderer_hide_overlay(&self) {
        rec(&self.log, json!(["renderer.hideOverlay"]));
    }
    fn renderer_render_now(&self) {
        rec(&self.log, json!(["renderer.renderNow"]));
    }
    fn renderer_add_child_by_name(&self, name: &str) {
        rec(
            &self.log,
            json!(["renderer.addChild", { "container": name, "children": [] }]),
        );
    }
    fn component_handle_input(&self, component: &ComponentRef, data: &str) -> bool {
        if component.kind == "target" {
            rec(&self.log, json!(["target.handleInput", data]));
            return true;
        }
        false
    }
}

struct RecEditor {
    log: Log,
    name: &'static str,
    text: Mutex<String>,
}

impl RecEditor {
    fn new(log: Log, name: &'static str) -> Self {
        Self {
            log,
            name,
            text: Mutex::new(String::new()),
        }
    }
}

impl ShellEditor for RecEditor {
    fn get_text(&self) -> String {
        self.text.lock().expect("text").clone()
    }
    fn get_expanded_text(&self) -> String {
        self.get_text()
    }
    fn set_text(&self, text: &str) {
        rec(&self.log, json!([format!("{}.setText", self.name), text]));
        *self.text.lock().expect("text") = text.to_string();
    }
    fn add_to_history(&self, text: &str) {
        rec(
            &self.log,
            json!([format!("{}.addToHistory", self.name), text]),
        );
    }
    fn insert_text_at_cursor(&self, text: &str) {
        rec(
            &self.log,
            json!([format!("{}.insertTextAtCursor", self.name), text]),
        );
    }
    fn set_border_color(&self, border: EditorBorder) {
        rec(
            &self.log,
            json!([format!("{}.borderColor", self.name), border.tag()]),
        );
    }
    fn border_color(&self) -> Option<String> {
        None
    }
    fn handle_input(&self, data: &str) {
        rec(
            &self.log,
            json!([format!("{}.handleInput", self.name), data]),
        );
    }
    fn set_working_status_indicator(&self, indicator: Option<ComponentRef>) {
        rec(
            &self.log,
            json!([
                format!("{}.setWorkingStatusIndicator", self.name),
                indicator
                    .map(|i| json!({ "kind": i.kind }))
                    .unwrap_or(json!("undefined"))
            ]),
        );
    }
    fn set_autocomplete_provider(&self) {
        rec(
            &self.log,
            json!([format!("{}.setAutocompleteProvider", self.name), "provider"]),
        );
    }
    fn set_padding_x(&self, px: i64) {
        rec(&self.log, json!([format!("{}.setPaddingX", self.name), px]));
    }
    fn set_autocomplete_max_visible(&self, n: i64) {
        rec(
            &self.log,
            json!([format!("{}.setAutocompleteMaxVisible", self.name), n]),
        );
    }
    fn get_padding_x(&self) -> i64 {
        2
    }
    fn get_autocomplete_max_visible(&self) -> i64 {
        6
    }
    fn on_action(&self, action: &'static str) {
        rec(
            &self.log,
            json!([format!("{}.onAction", self.name), action, "handler"]),
        );
    }
    fn set_on_escape(&self) {}
    fn set_on_ctrl_d(&self) {}
    fn set_on_submit(&self) {}
    fn set_on_change(&self) {}
    fn set_on_paste_image(&self) {}
    fn set_on_extension_shortcut(&self, _enabled: bool) {}
    fn embeds_working_status(&self) -> bool {
        true
    }
}

struct RecSettings {
    shared_log: Log,
    quiet_startup: AtomicU8,
    warnings_anthropic: AtomicBool,
    hide_thinking_block: AtomicBool,
    /// Upper-oracle knobs (drive-set overrides; defaults keep the r20
    /// harness values).
    show_terminal_progress: AtomicBool,
    show_cache_miss_notices: AtomicBool,
    collapse_changelog: Mutex<Option<bool>>,
    double_escape_action: Mutex<Option<String>>,
    last_changelog_version: Mutex<Option<Option<String>>>,
    project_trusted: AtomicBool,
}

impl RecSettings {
    fn new(log: Log) -> Self {
        Self {
            shared_log: log,
            quiet_startup: AtomicU8::new(0),
            warnings_anthropic: AtomicBool::new(true),
            hide_thinking_block: AtomicBool::new(false),
            show_terminal_progress: AtomicBool::new(false),
            show_cache_miss_notices: AtomicBool::new(false),
            collapse_changelog: Mutex::new(None),
            double_escape_action: Mutex::new(None),
            last_changelog_version: Mutex::new(None),
            project_trusted: AtomicBool::new(true),
        }
    }
}

impl ShellSettings for RecSettings {
    fn quiet_startup(&self) -> QuietStartup {
        match self.quiet_startup.load(Ordering::SeqCst) {
            1 => QuietStartup::Header,
            2 => QuietStartup::Full,
            _ => QuietStartup::Off,
        }
    }
    fn show_terminal_progress(&self) -> bool {
        self.show_terminal_progress.load(Ordering::SeqCst)
    }
    fn double_escape_action(&self) -> String {
        self.double_escape_action
            .lock()
            .expect("double escape")
            .clone()
            .unwrap_or_else(|| "fork".to_string())
    }
    fn hide_thinking_block(&self) -> bool {
        self.hide_thinking_block.load(Ordering::SeqCst)
    }
    fn set_hide_thinking_block(&self, value: bool) {
        self.hide_thinking_block.store(value, Ordering::SeqCst);
        rec(
            &self.shared_log,
            json!(["settings.setHideThinkingBlock", value]),
        );
    }
    fn show_cache_miss_notices(&self) -> bool {
        self.show_cache_miss_notices.load(Ordering::SeqCst)
    }
    fn collapse_changelog(&self) -> bool {
        self.collapse_changelog
            .lock()
            .expect("collapse")
            .unwrap_or(true)
    }
    fn output_pad(&self) -> i64 {
        1
    }
    fn editor_padding_x(&self) -> i64 {
        2
    }
    fn autocomplete_max_visible(&self) -> i64 {
        6
    }
    fn clear_on_shrink(&self) -> bool {
        true
    }
    fn show_hardware_cursor(&self) -> bool {
        false
    }
    fn fullscreen_scrollbar(&self) -> bool {
        false
    }
    fn fullscreen_copy_on_select(&self) -> bool {
        false
    }
    fn fullscreen_exit_output(&self) -> String {
        "transcript".to_string()
    }
    fn code_block_indent(&self) -> String {
        "  ".to_string()
    }
    fn enable_skill_commands(&self) -> bool {
        true
    }
    fn last_changelog_version(&self) -> Option<String> {
        self.last_changelog_version
            .lock()
            .expect("last changelog")
            .clone()
            .unwrap_or_else(|| Some("1.0.0".to_string()))
    }
    fn set_last_changelog_version(&self, version: &str) {
        *self.last_changelog_version.lock().expect("last changelog") =
            Some(Some(version.to_string()));
        rec(
            &self.shared_log,
            json!(["settings.setLastChangelogVersion", version]),
        );
    }
    fn show_images(&self) -> bool {
        false
    }
    fn image_width_cells(&self) -> i64 {
        20
    }
    fn project_trusted(&self) -> bool {
        self.project_trusted.load(Ordering::SeqCst)
    }
    fn http_idle_timeout_ms(&self) -> Option<u64> {
        Some(30000)
    }
    fn default_provider(&self) -> Option<String> {
        Some("anthropic".to_string())
    }
    fn default_model(&self) -> Option<String> {
        Some("claude-opus-4-8".to_string())
    }
    fn default_thinking_level(&self) -> Option<ThinkingLevel> {
        None
    }
    fn enabled_models(&self) -> Option<Vec<String>> {
        None
    }
    fn branch_summary_skip_prompt(&self) -> bool {
        false
    }
    fn external_editor_command(&self) -> String {
        "vim".to_string()
    }
    fn warnings_anthropic_extra_usage(&self) -> bool {
        self.warnings_anthropic.load(Ordering::SeqCst)
    }
    fn image_auto_resize(&self) -> bool {
        true
    }
    fn block_images(&self) -> bool {
        false
    }
    fn transport(&self) -> String {
        "auto".to_string()
    }
    fn default_project_trust(&self) -> String {
        "ask".to_string()
    }
    fn tree_filter_mode(&self) -> String {
        "all".to_string()
    }
    fn enable_install_telemetry(&self) -> bool {
        false
    }
    fn mermaid_rendering_mode(&self) -> String {
        "render".to_string()
    }
    fn theme(&self) -> String {
        "dark".to_string()
    }
    fn set(&self, name: &str, value: Value) {
        // `Value::Null` carries the JS `undefined` sentinel here.
        let rendered = if value.is_null() {
            json!("undefined")
        } else {
            render_arg(&value)
        };
        rec(
            &self.shared_log,
            json!([format!("settings.{name}"), rendered]),
        );
    }
}

struct RecSessionManager {
    tree: Mutex<Vec<(String, String)>>,
    leaf_id: Mutex<Option<String>>,
    session_name: Mutex<Option<String>>,
    /// Upper-oracle knobs.
    persisted: AtomicBool,
    context_entries: Mutex<Vec<SessionEntry>>,
    entries: Mutex<Vec<SessionEntry>>,
}

impl RecSessionManager {
    fn new() -> Self {
        Self {
            tree: Mutex::new(Vec::new()),
            leaf_id: Mutex::new(Some("leaf-1".to_string())),
            session_name: Mutex::new(None),
            persisted: AtomicBool::new(true),
            context_entries: Mutex::new(Vec::new()),
            entries: Mutex::new(Vec::new()),
        }
    }
}

impl ShellSessionManager for RecSessionManager {
    fn cwd(&self) -> String {
        "/work/project".to_string()
    }
    fn is_persisted(&self) -> bool {
        self.persisted.load(Ordering::SeqCst)
    }
    fn session_file(&self) -> Option<String> {
        Some("/home/u/.pi/agent/sessions/--work--project/abc123.jsonl".to_string())
    }
    fn session_id(&self) -> String {
        "abc123".to_string()
    }
    fn session_dir(&self) -> String {
        "/home/u/.pi/agent/sessions/--work--project".to_string()
    }
    fn uses_default_session_dir(&self) -> bool {
        true
    }
    fn session_name(&self) -> Option<String> {
        self.session_name.lock().expect("name").clone()
    }
    fn build_context_entries(&self) -> Vec<SessionEntry> {
        self.context_entries
            .lock()
            .expect("context entries")
            .clone()
    }
    fn entries(&self) -> Vec<SessionEntry> {
        self.entries.lock().expect("entries").clone()
    }
    fn branch(&self) -> Vec<SessionEntry> {
        Vec::new()
    }
    fn tree(&self) -> Vec<(String, String)> {
        self.tree.lock().expect("tree").clone()
    }
    fn leaf_id(&self) -> Option<String> {
        self.leaf_id.lock().expect("leaf").clone()
    }
    fn append_label_change(&self, _entry_id: &str, _label: &str) {}
    fn append_session_info(&self, _name: &str) {}
}

struct RecModelRuntime {
    log: Log,
    snapshot: Mutex<Vec<ModelRef>>,
    providers: Mutex<Vec<Value>>,
    check_auth: Mutex<Option<String>>,
    auth_key: Mutex<Option<String>>,
    credentials: Mutex<Result<Vec<Value>, String>>,
    /// Scenario checkAuth/getAuth/listCredentials overrides replace the
    /// logging defaults wholesale (their logs disappear, like the harness).
    auth_calls_logged: AtomicBool,
    credentials_logged: AtomicBool,
    /// The harness `fakeModelRuntime` has no `getProvider` member; set for
    /// `logout.selector` so `get_provider_name` throws the stub's TypeError.
    get_provider_missing: AtomicBool,
}

impl RecModelRuntime {
    fn new(log: Log) -> Self {
        Self {
            log,
            snapshot: Mutex::new(vec![
                ModelRef {
                    provider: "anthropic".to_string(),
                    id: "claude-opus-4-8".to_string(),
                    name: Some("Claude Opus 4.8".to_string()),
                    api: None,
                    reasoning: false,
                },
                ModelRef {
                    provider: "openai".to_string(),
                    id: "gpt-5.5".to_string(),
                    name: Some("GPT-5.5".to_string()),
                    api: None,
                    reasoning: false,
                },
            ]),
            providers: Mutex::new(vec![
                json!({"id": "anthropic", "name": "Anthropic", "auth": {"oauth": "claude", "apiKey": true}}),
                json!({"id": "openai", "name": "OpenAI", "auth": {"apiKey": true}}),
            ]),
            check_auth: Mutex::new(Some("api_key".to_string())),
            auth_key: Mutex::new(Some("sk-ant-oat-xyz".to_string())),
            credentials: Mutex::new(Ok(vec![json!({
                "providerId": "anthropic",
                "type": "oauth",
            })])),
            auth_calls_logged: AtomicBool::new(true),
            credentials_logged: AtomicBool::new(true),
            get_provider_missing: AtomicBool::new(false),
        }
    }
}

impl ShellModelRuntime for RecModelRuntime {
    fn available_snapshot(&self) -> Vec<ModelRef> {
        self.snapshot.lock().expect("snapshot").clone()
    }
    fn providers(&self) -> Vec<Value> {
        self.providers.lock().expect("providers").clone()
    }
    fn provider_auth_status(&self, provider_id: &str) -> (bool, Option<String>, Option<String>) {
        if provider_id == "anthropic" {
            (
                true,
                Some("subscription".to_string()),
                Some("auth.json".to_string()),
            )
        } else {
            (false, None, None)
        }
    }
    fn is_using_oauth(&self, provider_id: &str) -> bool {
        provider_id == "anthropic"
    }
    fn check_auth(&self, provider_id: &str) -> futures::future::BoxFuture<'_, Option<String>> {
        let log = self.log.clone();
        let provider_id = provider_id.to_string();
        let result = self.check_auth.lock().expect("check").clone();
        let logged = self.auth_calls_logged.load(Ordering::SeqCst);
        Box::pin(async move {
            if logged {
                rec(&log, json!(["modelRuntime.checkAuth", provider_id]));
            }
            result
        })
    }
    fn get_auth_api_key(
        &self,
        provider_id: &str,
    ) -> futures::future::BoxFuture<'_, Option<String>> {
        let log = self.log.clone();
        let provider_id = provider_id.to_string();
        let key = self.auth_key.lock().expect("key").clone();
        let logged = self.auth_calls_logged.load(Ordering::SeqCst);
        Box::pin(async move {
            if logged {
                rec(&log, json!(["modelRuntime.getAuth", provider_id]));
            }
            key
        })
    }
    fn list_credentials(&self) -> futures::future::BoxFuture<'_, Result<Vec<Value>, String>> {
        let log = self.log.clone();
        let credentials = self.credentials.lock().expect("credentials").clone();
        let logged = self.credentials_logged.load(Ordering::SeqCst);
        Box::pin(async move {
            if logged {
                rec(&log, json!(["modelRuntime.listCredentials"]));
            }
            credentials
        })
    }
    fn get_provider_name(&self, _provider_id: &str) -> Result<Option<String>, String> {
        if self.get_provider_missing.load(Ordering::SeqCst) {
            // The harness stub runtime has no `getProvider` member; the
            // upstream member call throws this exact TypeError.
            return Err("this.session.modelRuntime.getProvider is not a function".to_string());
        }
        // A real runtime resolves the provider's display name; the fixture
        // default carries none, so `getLogoutProviderOptions` falls back to
        // the provider id (the pre-seam port behavior).
        Ok(None)
    }
    fn logout(&self, _provider_id: &str) -> futures::future::BoxFuture<'_, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
    fn login(
        &self,
        provider_id: &str,
        method: &str,
        _callbacks: LoginCallbacks<'_>,
    ) -> futures::future::BoxFuture<'_, Result<(), LoginError>> {
        let log = self.log.clone();
        let provider_id = provider_id.to_string();
        let method = method.to_string();
        Box::pin(async move {
            rec(
                &log,
                json!(["modelRuntime.login", provider_id, method, {
                    "signal": {
                        "aborted": false,
                        "addEventListener": "function",
                        "removeEventListener": "function",
                    },
                    "prompt": "function",
                    "notify": "function",
                }]),
            );
            Ok(())
        })
    }
    fn refresh(
        &self,
        providers: Option<Vec<String>>,
    ) -> futures::future::BoxFuture<'_, RefreshResult> {
        let log = self.log.clone();
        Box::pin(async move {
            match providers {
                // The model-catalog coordinator path (`refreshModelCatalogs`).
                None => rec(&log, json!(["refreshModelCatalogs", "signal"])),
                Some(list) => rec(
                    &log,
                    json!(["modelRuntime.refresh", { "providers": list, "signal": {} }]),
                ),
            }
            RefreshResult {
                aborted: false,
                errors: Vec::new(),
            }
        })
    }
    fn get_error(&self) -> Option<String> {
        None
    }
}

struct RecResources {
    skills: Mutex<ResourceGroupRead>,
    prompts: Mutex<ResourceGroupRead>,
    themes: Mutex<ResourceGroupRead>,
    extensions: Mutex<(Vec<LoadedResource>, Vec<(String, String)>)>,
    system_prompt_source: Mutex<Option<LoadedResource>>,
    append_sources: Mutex<Vec<LoadedResource>>,
    agents_files: Mutex<Vec<LoadedResource>>,
}

impl Default for RecResources {
    fn default() -> Self {
        Self {
            skills: Mutex::new(ResourceGroupRead::default()),
            prompts: Mutex::new(ResourceGroupRead::default()),
            themes: Mutex::new(ResourceGroupRead::default()),
            extensions: Mutex::new((Vec::new(), Vec::new())),
            system_prompt_source: Mutex::new(None),
            append_sources: Mutex::new(Vec::new()),
            agents_files: Mutex::new(Vec::new()),
        }
    }
}

impl ShellResources for RecResources {
    fn skills(&self) -> ResourceGroupRead {
        self.skills.lock().expect("skills").clone()
    }
    fn prompts(&self) -> ResourceGroupRead {
        self.prompts.lock().expect("prompts").clone()
    }
    fn themes(&self) -> ResourceGroupRead {
        self.themes.lock().expect("themes").clone()
    }
    fn extensions(&self) -> (Vec<LoadedResource>, Vec<(String, String)>) {
        self.extensions.lock().expect("extensions").clone()
    }
    fn system_prompt_source(&self) -> Option<LoadedResource> {
        self.system_prompt_source.lock().expect("sys").clone()
    }
    fn append_system_prompt_sources(&self) -> Vec<LoadedResource> {
        self.append_sources.lock().expect("append").clone()
    }
    fn agents_files(&self) -> Vec<LoadedResource> {
        self.agents_files.lock().expect("agents").clone()
    }
    fn prompt_templates(&self) -> Vec<LoadedResource> {
        Vec::new()
    }
}

struct RecExtensions {
    log: Log,
    command_diagnostics: Arc<Mutex<Vec<ResourceDiagnostic>>>,
}

impl ShellExtensionSurface for RecExtensions {
    fn has_command(&self, name: &str) -> bool {
        name == "extcmd" || name == "extcmd2"
    }
    fn registered_commands(&self) -> Vec<ExtensionCommandInfo> {
        Vec::new()
    }
    fn command_diagnostics(&self) -> Vec<ResourceDiagnostic> {
        self.command_diagnostics
            .lock()
            .expect("command diagnostics")
            .clone()
    }
    fn shortcut_diagnostics(&self) -> Vec<ResourceDiagnostic> {
        Vec::new()
    }
    fn markdown_transformers(&self) -> Vec<String> {
        vec!["extTransformer".to_string()]
    }
    fn has_entry_renderer(&self, _custom_type: &str) -> bool {
        false
    }
    fn has_message_renderer(&self, _custom_type: &str) -> bool {
        false
    }
    fn emit(&self, tuple: Value) {
        rec(&self.log, tuple);
    }
}

/// The `emitUserBash` fixture behavior.
enum ShortcutsUserBash {
    /// No interception (fall through to local execution).
    PassThrough,
    /// The extension returns a full result.
    FullResult(BashOutcome),
    /// The extension runner throws.
    Throws,
}

struct RecShortcuts {
    log: Log,
    shortcuts: Mutex<Vec<ExtensionShortcut>>,
    user_bash: Mutex<ShortcutsUserBash>,
}

impl RecShortcuts {
    fn new(log: Log) -> Self {
        Self {
            log,
            shortcuts: Mutex::new(Vec::new()),
            user_bash: Mutex::new(ShortcutsUserBash::PassThrough),
        }
    }
}

impl ShellShortcutSurface for RecShortcuts {
    fn shortcuts(&self) -> Vec<ExtensionShortcut> {
        self.shortcuts.lock().expect("shortcuts").clone()
    }
    fn emit_user_bash(
        &self,
        command: &str,
        exclude_from_context: bool,
        cwd: &str,
    ) -> futures::future::BoxFuture<'_, Result<UserBashOutcome, String>> {
        let log = self.log.clone();
        let command = command.to_string();
        let exclude = exclude_from_context;
        let cwd = cwd.to_string();
        let behavior = match &*self.user_bash.lock().expect("bash") {
            ShortcutsUserBash::PassThrough => ShortcutsUserBash::PassThrough,
            ShortcutsUserBash::FullResult(result) => ShortcutsUserBash::FullResult(result.clone()),
            ShortcutsUserBash::Throws => ShortcutsUserBash::Throws,
        };
        Box::pin(async move {
            // Scenario overrides replace the default stub wholesale; only the
            // pass-through default records the emit.
            if matches!(behavior, ShortcutsUserBash::PassThrough) {
                rec(
                    &log,
                    json!([
                        "extensionRunner.emitUserBash",
                        {
                            "type": "user_bash",
                            "command": command,
                            "excludeFromContext": exclude,
                            "cwd": cwd,
                        }
                    ]),
                );
            }
            match behavior {
                ShortcutsUserBash::PassThrough => Ok(UserBashOutcome::default()),
                ShortcutsUserBash::FullResult(result) => Ok(UserBashOutcome {
                    result: Some(result),
                }),
                ShortcutsUserBash::Throws => Err("extension crashed".to_string()),
            }
        })
    }
}

struct RecSession {
    log: Log,
    streaming: AtomicBool,
    compacting: AtomicBool,
    model: Mutex<Option<ModelRef>>,
    thinking: Mutex<ThinkingLevel>,
    user_messages: Mutex<Vec<ForkableUserMessage>>,
    last_assistant_text: Mutex<Option<String>>,
    stats: Mutex<SessionStats>,
    /// Exact `session.setModel` describe override (the JS drivers pass minimal
    /// model objects; the first `set_model` consumes it).
    set_model_log: Mutex<Option<Value>>,
    /// `set_model` rejection (logged nowhere, like the throwing fixture).
    set_model_error: Mutex<Option<String>>,
    export_jsonl_error: Mutex<Option<String>>,
    export_html: Mutex<Result<String, String>>,
    execute_bash: Mutex<Result<BashOutcome, String>>,
    reload: Mutex<Result<(), String>>,
    navigate_result: Mutex<NavigateOutcome>,
    command_diagnostics: Arc<Mutex<Vec<ResourceDiagnostic>>>,
    runtime: Arc<RecModelRuntime>,
    resources: Arc<RecResources>,
    shortcuts: Arc<RecShortcuts>,
}

impl RecSession {
    fn new(
        log: Log,
        runtime: Arc<RecModelRuntime>,
        resources: Arc<RecResources>,
        shortcuts: Arc<RecShortcuts>,
    ) -> Self {
        Self {
            log,
            streaming: AtomicBool::new(false),
            compacting: AtomicBool::new(false),
            model: Mutex::new(None),
            thinking: Mutex::new(ThinkingLevel::Medium),
            user_messages: Mutex::new(Vec::new()),
            last_assistant_text: Mutex::new(Some("last assistant text".to_string())),
            stats: Mutex::new(SessionStats::default()),
            set_model_log: Mutex::new(None),
            set_model_error: Mutex::new(None),
            export_jsonl_error: Mutex::new(None),
            export_html: Mutex::new(Ok("/out.html".to_string())),
            execute_bash: Mutex::new(Ok(BashOutcome {
                exit_code: Some(0),
                cancelled: false,
                output: "out".to_string(),
                truncated: false,
                full_output_path: None,
            })),
            reload: Mutex::new(Ok(())),
            navigate_result: Mutex::new(NavigateOutcome::default()),
            command_diagnostics: Arc::new(Mutex::new(Vec::new())),
            runtime,
            resources,
            shortcuts,
        }
    }
}

impl ShellSession for RecSession {
    fn is_streaming(&self) -> bool {
        self.streaming.load(Ordering::SeqCst)
    }
    fn is_compacting(&self) -> bool {
        self.compacting.load(Ordering::SeqCst)
    }
    fn is_bash_running(&self) -> bool {
        false
    }
    fn is_idle(&self) -> bool {
        !self.is_streaming()
    }
    fn thinking_level(&self) -> ThinkingLevel {
        *self.thinking.lock().expect("thinking")
    }
    fn retry_attempt(&self) -> u32 {
        0
    }
    fn pending_message_count(&self) -> usize {
        0
    }
    fn scoped_models(&self) -> Vec<crate::coding_agent::agent_session::ScopedModel> {
        Vec::new()
    }
    fn steering_messages(&self) -> Vec<String> {
        Vec::new()
    }
    fn follow_up_messages(&self) -> Vec<String> {
        Vec::new()
    }
    fn clear_queue(&self) -> (Vec<String>, Vec<String>) {
        rec(
            &self.log,
            json!(["session.clearQueue", { "steering": [], "followUp": [] }]),
        );
        (Vec::new(), Vec::new())
    }
    fn prompt(
        &self,
        text: String,
        streaming_behavior: Option<StreamingDelivery>,
    ) -> futures::future::BoxFuture<'_, Result<(), AgentSessionError>> {
        let log = self.log.clone();
        Box::pin(async move {
            rec(
                &log,
                json!(["session.prompt", text, streaming_behavior
                    .map(|b| json!({ "streamingBehavior": match b { StreamingDelivery::Steer => "steer", StreamingDelivery::FollowUp => "followUp" } }))
                    .unwrap_or(Value::Null)]),
            );
            Ok(())
        })
    }
    fn steer(&self, text: String) -> futures::future::BoxFuture<'_, Result<(), AgentSessionError>> {
        let log = self.log.clone();
        Box::pin(async move {
            rec(&log, json!(["session.steer", text]));
            Ok(())
        })
    }
    fn follow_up(
        &self,
        text: String,
    ) -> futures::future::BoxFuture<'_, Result<(), AgentSessionError>> {
        let log = self.log.clone();
        Box::pin(async move {
            rec(&log, json!(["session.followUp", text]));
            Ok(())
        })
    }
    fn abort(&self) {
        rec(&self.log, json!(["session.abort"]));
    }
    fn abort_bash(&self) {
        rec(&self.log, json!(["session.abortBash"]));
    }
    fn abort_compaction(&self) {
        rec(&self.log, json!(["session.abortCompaction"]));
    }
    fn abort_retry(&self) {
        rec(&self.log, json!(["session.abortRetry"]));
    }
    fn cycle_thinking_level(&self) -> Option<ThinkingLevel> {
        None
    }
    fn cycle_model(
        &self,
        _direction: CycleDirection,
    ) -> futures::future::BoxFuture<'_, Result<Option<ModelCycleResult>, AgentSessionError>> {
        Box::pin(async { Ok(None) })
    }
    fn extensions(&self) -> Arc<dyn ShellExtensionSurface> {
        Arc::new(RecExtensions {
            log: self.log.clone(),
            command_diagnostics: self.command_diagnostics.clone(),
        })
    }
    fn maybe_warn_anthropic_subscription_auth(&self, _provider: Option<&str>) {}
    fn subscribe(&self) -> u64 {
        rec(&self.log, json!(["session.subscribe"]));
        1
    }
    fn unsubscribe(&self, _slot: u64) {
        rec(&self.log, json!(["session.unsubscribe"]));
    }
    fn model(&self) -> Option<ModelRef> {
        self.model.lock().expect("model").clone()
    }
    fn model_runtime(&self) -> Arc<dyn ShellModelRuntime> {
        self.runtime.clone()
    }
    fn resources(&self) -> Arc<dyn ShellResources> {
        self.resources.clone()
    }
    fn shortcuts(&self) -> Arc<dyn ShellShortcutSurface> {
        self.shortcuts.clone()
    }
    fn available_thinking_levels(&self) -> Vec<ThinkingLevel> {
        vec![
            ThinkingLevel::Off,
            ThinkingLevel::Minimal,
            ThinkingLevel::Low,
            ThinkingLevel::Medium,
            ThinkingLevel::High,
        ]
    }
    fn set_model(
        &self,
        model: &ModelRef,
        persist: bool,
    ) -> futures::future::BoxFuture<'_, Result<(), String>> {
        let log = self.log.clone();
        let model = model.clone();
        let override_describe = self.set_model_log.lock().expect("override").take();
        let error = self.set_model_error.lock().expect("error").clone();
        Box::pin(async move {
            if let Some(error) = error {
                // The throwing fixture records nothing.
                return Err(error);
            }
            *self.model.lock().expect("model") = Some(model.clone());
            let described = override_describe.unwrap_or_else(|| model.to_value());
            rec(
                &log,
                json!(["session.setModel", described, { "persist": persist }]),
            );
            Ok(())
        })
    }
    fn set_thinking_level(&self, level: ThinkingLevel, persist: bool) -> Result<(), String> {
        // The upstream fixture's `thinkingLevel` is a static field; only the
        // call is recorded.
        let _ = level;
        rec(
            &self.log,
            json!([
                "session.setThinkingLevel",
                super::shell_lower::thinking_level_lower(&level),
                { "persist": persist }
            ]),
        );
        Ok(())
    }
    fn set_scoped_models(&self, models: &[ModelRef]) {
        let described: Vec<Value> = models
            .iter()
            .map(|m| {
                json!({
                    "model": m.to_value(),
                    "thinkingLevel": "undefined",
                })
            })
            .collect();
        rec(&self.log, json!(["session.setScopedModels", described]));
    }
    fn auto_compaction_enabled(&self) -> bool {
        true
    }
    fn set_auto_compaction_enabled(&self, enabled: bool) {
        rec(
            &self.log,
            json!(["session.setAutoCompactionEnabled", enabled]),
        );
    }
    fn steering_mode(&self) -> Value {
        json!("interrupt")
    }
    fn follow_up_mode(&self) -> Value {
        json!("queue")
    }
    fn set_steering_mode(&self, _mode: Value) {}
    fn set_follow_up_mode(&self, _mode: Value) {}
    fn user_messages_for_forking(&self) -> Vec<ForkableUserMessage> {
        self.user_messages.lock().expect("fork messages").clone()
    }
    fn session_stats(&self) -> SessionStats {
        self.stats.lock().expect("stats").clone()
    }
    fn last_assistant_text(&self) -> Option<String> {
        self.last_assistant_text.lock().expect("last").clone()
    }
    fn set_session_name(&self, name: &str) {
        rec(&self.log, json!(["session.setSessionName", name]));
    }
    fn emit(&self, tuple: Value) {
        rec(&self.log, tuple);
    }
    fn navigate_tree(
        &self,
        entry_id: &str,
        summarize: bool,
        custom_instructions: Option<&str>,
    ) -> futures::future::BoxFuture<'_, Result<NavigateOutcome, String>> {
        let log = self.log.clone();
        let entry_id = entry_id.to_string();
        let custom = custom_instructions.map(str::to_string);
        let result = self.navigate_result.lock().expect("navigate").clone();
        Box::pin(async move {
            rec(
                &log,
                json!(["session.navigateTree", entry_id, {
                    "summarize": summarize,
                    "customInstructions": custom
                        .map(Value::from)
                        .unwrap_or(json!("undefined")),
                }]),
            );
            Ok(result)
        })
    }
    fn abort_branch_summary(&self) {}
    fn compact(
        &self,
        custom_instructions: Option<&str>,
    ) -> futures::future::BoxFuture<'_, Result<(), String>> {
        let log = self.log.clone();
        let custom = custom_instructions.map(str::to_string);
        Box::pin(async move {
            rec(
                &log,
                json!([
                    "session.compact",
                    custom.map(Value::from).unwrap_or(json!("undefined"))
                ]),
            );
            Ok(())
        })
    }
    fn execute_bash(
        &self,
        command: &str,
        exclude_from_context: bool,
        _chunk_sink: &dyn Fn(&str),
    ) -> futures::future::BoxFuture<'_, Result<BashOutcome, String>> {
        let log = self.log.clone();
        let command = command.to_string();
        let exclude = exclude_from_context;
        let result = self.execute_bash.lock().expect("bash").clone();
        Box::pin(async move {
            // The throwing scenario override replaces the logging stub.
            if result.is_ok() {
                rec(
                    &log,
                    json!(["session.executeBash", command, "function", {
                        "excludeFromContext": exclude,
                        "operations": "undefined",
                    }]),
                );
            }
            result
        })
    }
    fn record_bash_result(&self, command: &str, result: &BashOutcome, exclude_from_context: bool) {
        rec(
            &self.log,
            json!(["session.recordBashResult", command, {
                "output": result.output,
                "exitCode": result.exit_code,
                "cancelled": result.cancelled,
            }, { "excludeFromContext": exclude_from_context }]),
        );
    }
    fn reload(
        &self,
        before_session_start: Option<&dyn Fn()>,
    ) -> futures::future::BoxFuture<'_, Result<(), String>> {
        // Recorded at call time (the shell awaits immediately, so the log
        // order is unchanged; the hook is not `Send` and cannot cross into
        // the boxed future).
        match &*self.reload.lock().expect("reload") {
            Ok(()) => {
                rec(
                    &self.log,
                    json!(["session.reload", { "beforeSessionStart": "function" }]),
                );
                if let Some(hook) = before_session_start {
                    hook();
                }
                Box::pin(async { Ok(()) })
            }
            Err(error) => {
                let error = error.clone();
                Box::pin(async move { Err(error) })
            }
        }
    }
    fn build_bug_report_bundle(
        &self,
        options: super::bug_report::BugReportOptions,
        summary: Option<String>,
    ) -> futures::future::BoxFuture<'_, Result<super::interactive_mode::BugReportOutcome, String>>
    {
        let _ = (options, summary);
        Box::pin(async {
            Ok(super::interactive_mode::BugReportOutcome {
                report_id: "bug-1".to_string(),
                created_at: "2026-01-01T00:00:00.000Z".to_string(),
                zip_path: Some("pi-bug-report-bug-1.zip".to_string()),
                crash_count: 0,
            })
        })
    }
    fn export_to_jsonl(&self, path: &str) -> Result<String, String> {
        let error = self.export_jsonl_error.lock().expect("export").clone();
        match error {
            // The throwing scenario override replaces the logging stub.
            Some(error) => Err(error),
            None => {
                rec(&self.log, json!(["session.exportToJsonl", path]));
                Ok(path.to_string())
            }
        }
    }
    fn export_to_html(
        &self,
        path: Option<&str>,
        theme_name: &str,
    ) -> futures::future::BoxFuture<'_, Result<String, String>> {
        let log = self.log.clone();
        let path = path.map(str::to_string);
        let theme_name = theme_name.to_string();
        let result = self.export_html.lock().expect("html").clone();
        Box::pin(async move {
            rec(
                &log,
                json!([
                    "session.exportToHtml",
                    path.map(Value::from).unwrap_or(json!("undefined")),
                    { "themeName": theme_name },
                ]),
            );
            result
        })
    }
    fn tool_definition(&self, name: &str) -> Value {
        json!({ "name": name, "builtIn": true })
    }
    fn context_usage(&self) -> Option<Value> {
        None
    }
    fn system_prompt(&self) -> String {
        "sys".to_string()
    }
    fn messages(&self) -> Vec<AgentMessage> {
        Vec::new()
    }
    fn wait_for_idle(&self) -> futures::future::BoxFuture<'_, ()> {
        Box::pin(async {})
    }
    fn bind_extensions(&self, _context: Value) -> futures::future::BoxFuture<'_, ()> {
        Box::pin(async {})
    }
    fn detect_cache_miss(&self, _message: &AgentMessage) -> Option<CacheMiss> {
        None
    }
    fn collect_cache_misses(&self) -> Vec<(Value, CacheMiss)> {
        Vec::new()
    }
}

struct RecHost {
    log: Log,
    switch_behavior: Mutex<SwitchBehavior>,
    import_behavior: Mutex<ImportBehavior>,
    /// The `cmd.clear.cancelled` scenario stubs `newSession` wholesale (the
    /// logging default disappears, like the harness).
    new_session_cancelled: AtomicBool,
}

#[derive(Clone)]
enum SwitchBehavior {
    Ok,
    MissingCwd(String),
}

#[derive(Clone)]
enum ImportBehavior {
    Ok,
    MissingCwd(String),
    NotFound(String),
}

impl RecHost {
    fn new(log: Log) -> Self {
        Self {
            log,
            switch_behavior: Mutex::new(SwitchBehavior::Ok),
            import_behavior: Mutex::new(ImportBehavior::Ok),
            new_session_cancelled: AtomicBool::new(false),
        }
    }
}

impl ShellHost for RecHost {
    fn set_before_session_invalidate(&self, _hook: Option<Box<dyn Fn() + Send + Sync>>) {}
    fn set_rebind_session(
        &self,
        _hook: Option<Box<dyn Fn() -> futures::future::BoxFuture<'static, ()> + Send + Sync>>,
    ) {
    }
    fn dispose(&self) -> futures::future::BoxFuture<'_, ()> {
        let log = self.log.clone();
        Box::pin(async move {
            rec(&log, json!(["runtimeHost.dispose"]));
        })
    }
    fn new_session(
        &self,
        _options: Value,
    ) -> futures::future::BoxFuture<'_, Result<NewSessionOutcome, String>> {
        let log = self.log.clone();
        let cancelled = self.new_session_cancelled.load(Ordering::SeqCst);
        Box::pin(async move {
            if cancelled {
                // The scenario replaces the logging stub wholesale.
                return Ok(NewSessionOutcome { cancelled: true });
            }
            rec(&log, json!(["runtimeHost.newSession"]));
            Ok(NewSessionOutcome { cancelled: false })
        })
    }
    fn fork(
        &self,
        entry_id: &str,
        options: Value,
    ) -> futures::future::BoxFuture<'_, Result<ForkOutcome, String>> {
        let log = self.log.clone();
        let entry_id = entry_id.to_string();
        Box::pin(async move {
            // The upstream host call passes no options for plain forks.
            if options.is_null() {
                rec(&log, json!(["runtimeHost.fork", entry_id]));
            } else {
                rec(&log, json!(["runtimeHost.fork", entry_id, options]));
            }
            Ok(ForkOutcome {
                cancelled: false,
                selected_text: Some("picked".to_string()),
            })
        })
    }
    fn switch_session(
        &self,
        session_path: &str,
        _cwd_override: Option<&str>,
    ) -> futures::future::BoxFuture<'_, Result<ForkOutcome, HostError>> {
        let log = self.log.clone();
        let path = session_path.to_string();
        let behavior = match &*self.switch_behavior.lock().expect("switch") {
            SwitchBehavior::Ok => SwitchBehavior::Ok,
            SwitchBehavior::MissingCwd(fallback) => SwitchBehavior::MissingCwd(fallback.clone()),
        };
        let guard_ok = _cwd_override.is_some();
        Box::pin(async move {
            // Behavior overrides replace the logging stub wholesale.
            if matches!(behavior, SwitchBehavior::Ok) {
                rec(
                    &log,
                    json!(["runtimeHost.switchSession", path, {
                        "withSession": "undefined",
                        "projectTrustContextFactory": "function",
                    }]),
                );
            }
            match behavior {
                SwitchBehavior::Ok => Ok(ForkOutcome {
                    cancelled: false,
                    selected_text: None,
                }),
                SwitchBehavior::MissingCwd(fallback) => {
                    // The override rejects only the un-guarded call; the
                    // fallback retry succeeds.
                    if guard_ok {
                        Ok(ForkOutcome {
                            cancelled: false,
                            selected_text: None,
                        })
                    } else {
                        Err(HostError::MissingSessionCwd {
                            fallback_cwd: fallback,
                        })
                    }
                }
            }
        })
    }
    fn import_from_jsonl(
        &self,
        path: &str,
        _cwd_override: Option<&str>,
    ) -> futures::future::BoxFuture<'_, Result<ForkOutcome, HostError>> {
        let log = self.log.clone();
        let path = path.to_string();
        let behavior = match &*self.import_behavior.lock().expect("import") {
            ImportBehavior::Ok => ImportBehavior::Ok,
            ImportBehavior::MissingCwd(fallback) => ImportBehavior::MissingCwd(fallback.clone()),
            ImportBehavior::NotFound(message) => ImportBehavior::NotFound(message.clone()),
        };
        let guard_ok = _cwd_override.is_some();
        Box::pin(async move {
            // Behavior overrides replace the logging stub wholesale.
            if matches!(behavior, ImportBehavior::Ok) {
                rec(&log, json!(["runtimeHost.importFromJsonl", path]));
            }
            match behavior {
                ImportBehavior::Ok => Ok(ForkOutcome {
                    cancelled: false,
                    selected_text: None,
                }),
                ImportBehavior::MissingCwd(fallback) => {
                    // The override rejects only the un-guarded call; the
                    // fallback retry succeeds.
                    if guard_ok {
                        Ok(ForkOutcome {
                            cancelled: false,
                            selected_text: None,
                        })
                    } else {
                        Err(HostError::MissingSessionCwd {
                            fallback_cwd: fallback,
                        })
                    }
                }
                ImportBehavior::NotFound(message) => Err(HostError::ImportFileNotFound(message)),
            }
        })
    }
    fn agent_dir(&self) -> String {
        "/home/u/.pi/agent".to_string()
    }
}

// ---------------------------------------------------------------------------
// The checkForPackageUpdates npm probe fixture
// ---------------------------------------------------------------------------

/// The scripted npm transport: `npm view <spec> version --json` answers the
/// fixed registry version (loopback stub; nothing spawns or touches the
/// network).
struct PmProbeRunner;

impl CommandRunner for PmProbeRunner {
    fn run(&self, command: &str, args: &[String], _cwd: Option<&str>) -> Result<(), CommandError> {
        Err(CommandError::new(format!(
            "unexpected spawn: {} {}",
            command,
            args.join(" ")
        )))
    }
    fn run_capture(
        &self,
        command: &str,
        args: &[String],
        _cwd: Option<&str>,
        _timeout_ms: Option<u64>,
        _extra_env: &[(String, String)],
    ) -> Result<String, CommandError> {
        if args.len() == 4 && args[0] == "view" && args[2] == "version" && args[3] == "--json" {
            return Ok("\"2.0.0\"".to_string());
        }
        Err(CommandError::new(format!(
            "unexpected capture: {} {}",
            command,
            args.join(" ")
        )))
    }
    fn run_sync(&self, command: &str, args: &[String]) -> Result<String, CommandError> {
        Err(CommandError::new(format!(
            "unexpected sync: {} {}",
            command,
            args.join(" ")
        )))
    }
}

/// User-scope settings seeded with the installed `npm:pkg-a` package.
struct PmProbeSettings;

impl SettingsManagerHandle for PmProbeSettings {
    fn global_settings(&self) -> SettingsData {
        SettingsData {
            packages: vec![PackageSourceEntry::Plain("npm:pkg-a".to_string())],
            ..SettingsData::default()
        }
    }
    fn project_settings(&self) -> SettingsData {
        SettingsData::default()
    }
    fn is_project_trusted(&self) -> bool {
        true
    }
    fn set_project_trusted(&self, _trusted: bool) {}
    fn npm_command(&self) -> Option<Vec<String>> {
        None
    }
    fn set_packages(&self, _packages: Vec<PackageSourceEntry>) {}
    fn set_project_packages(&self, _packages: Vec<PackageSourceEntry>) {}
}

static PM_PROBE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The `new DefaultPackageManager({cwd, agentDir, settingsManager})` +
/// `checkForAvailableUpdates()` fixture. Construction is byte-logged with the
/// shell-passed strings (the harness stub constructor's `LOG.push`); the REAL
/// [`DefaultPackageManager`] port then runs the probe over a throwaway
/// install tree (`npm/node_modules/pkg-a` @1.0.0 under a fresh agent dir)
/// with the scripted transport, exercising the settings → dedupe → parse →
/// install-path → `npm view` → semver pipeline end to end.
struct FixturePackageUpdates {
    log: Log,
}

impl FixturePackageUpdates {
    fn new(log: Log) -> Self {
        Self { log }
    }

    /// The harness `describeArg` of the fake settings manager (fixture at
    /// defaults; every member is a function).
    fn settings_manager_describe() -> Value {
        json!({
            "getQuietStartup": "function",
            "getShowTerminalProgress": "function",
            "getDoubleEscapeAction": "function",
            "getHideThinkingBlock": "function",
            "getShowCacheMissNotices": "function",
            "getCollapseChangelog": "function",
            "getOutputPad": "function",
            "getEditorPaddingX": "function",
            "getAutocompleteMaxVisible": "function",
            "getClearOnShrink": "function",
            "getShowHardwareCursor": "function",
            "getFullscreenScrollbar": "function",
            "getFullscreenCopyOnSelect": "function",
            "getFullscreenExitOutput": "function",
            "getCodeBlockIndent": "function",
            "getTerminalCapabilityOverrides": "function",
            "getHttpIdleTimeoutMs": "function",
            "getMermaidRenderingMode": "function",
            "getEnableSkillCommands": "function",
            "getLastChangelogVersion": "function",
            "getShowImages": "function",
            "getImageWidthCells": "function",
            "isProjectTrusted": "function",
            "getTheme": "function",
            "setLastChangelogVersion": "function",
            "setHideThinkingBlock": "function",
        })
    }
}

impl PackageUpdatesSource for FixturePackageUpdates {
    fn check_for_available_updates(
        &self,
        cwd: &str,
        agent_dir: &str,
    ) -> Result<Vec<PackageUpdate>, String> {
        rec(
            &self.log,
            json!(["new DefaultPackageManager", {
                "cwd": cwd,
                "agentDir": agent_dir,
                "settingsManager": Self::settings_manager_describe(),
            }]),
        );
        // Materialize the installed package in a throwaway tree (the logged
        // agent dir is the harness string, not a real path on this machine).
        let temp = std::env::temp_dir().join(format!(
            "pi-shell-oracle-pm-{}-{}",
            std::process::id(),
            PM_PROBE_COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let agent_root = temp.join("agent");
        let installed = agent_root.join("npm").join("node_modules").join("pkg-a");
        std::fs::create_dir_all(&installed).map_err(|error| error.to_string())?;
        std::fs::write(
            installed.join("package.json"),
            "{\"name\":\"pkg-a\",\"version\":\"1.0.0\"}",
        )
        .map_err(|error| error.to_string())?;
        std::fs::create_dir_all(temp.join("work")).map_err(|error| error.to_string())?;
        let manager = DefaultPackageManager::new(PackageManagerOptions {
            cwd: temp.join("work").to_string_lossy().to_string(),
            agent_dir: agent_root.to_string_lossy().to_string(),
            settings_manager: Arc::new(PmProbeSettings),
            command_runner: Some(Arc::new(PmProbeRunner)),
        });
        let result = manager
            .check_for_available_updates()
            .map_err(|error| error.message);
        let _ = std::fs::remove_dir_all(&temp);
        result
    }
}

struct RecPlatform {
    log: Log,
    now_iso: Mutex<String>,
}

impl RecPlatform {
    fn new(log: Log) -> Self {
        Self {
            log,
            now_iso: Mutex::new("2026-09-28T14:02:27.438Z".to_string()),
        }
    }
}

impl ShellPlatform for RecPlatform {
    fn exit(&self, code: i32) -> bool {
        rec(&self.log, json!(["process.exit", code]));
        // The r20 harness's exit stub throws (never returns); the shell's
        // post-exit statements stay unreachable in that family.
        false
    }
    fn stop_theme_watcher(&self) -> Result<(), String> {
        // The r20 drive defines `stopThemeWatcher` and records the call.
        rec(&self.log, json!(["stopThemeWatcher"]));
        Ok(())
    }
    fn is_windows(&self) -> bool {
        // The oracle harness ran on win32 (path.join shape, hotkeys note).
        true
    }
    fn now_ms(&self) -> i64 {
        1_790_604_147_438
    }
    fn stdout_is_tty(&self) -> bool {
        false
    }
    fn kill_tracked_detached_children(&self) {
        rec(&self.log, json!(["killTrackedDetachedChildren"]));
    }
    fn copy_to_clipboard(&self, text: &str) -> futures::future::BoxFuture<'_, Result<(), String>> {
        let log = self.log.clone();
        let text = text.to_string();
        Box::pin(async move {
            rec(&log, json!(["copyToClipboard", text]));
            Ok(())
        })
    }
    fn read_clipboard_text(&self) -> futures::future::BoxFuture<'_, Option<String>> {
        Box::pin(async { Some("clip text".to_string()) })
    }
    fn read_clipboard_image(&self) -> futures::future::BoxFuture<'_, Option<(String, Vec<u8>)>> {
        Box::pin(async { None })
    }
    fn pi_offline(&self) -> bool {
        false
    }
    fn has_trust_requiring_project_resources(&self, _cwd: &str) -> bool {
        false
    }
    fn register_signal_handlers(&self) -> Vec<u64> {
        Vec::new()
    }
    fn unregister_signal_handlers(&self, _ids: &[u64]) {}
    fn suspend(&self) -> Option<()> {
        Some(())
    }
    fn now_iso(&self) -> String {
        self.now_iso.lock().expect("now").clone()
    }
    fn basename(&self, path: &str) -> String {
        path.rsplit(['/', '\\'])
            .next()
            .unwrap_or_default()
            .to_string()
    }
    fn join_path(&self, parts: &[&str]) -> String {
        // win32 join: `path.join` normalizes separators on Windows and keeps
        // the leading separator of the first segment.
        let normalized: Vec<String> = parts.iter().map(|p| p.replace('/', "\\")).collect();
        normalized.join("\\")
    }
    fn file_exists(&self, _path: &str) -> bool {
        true
    }
}

struct RecChangelog;

impl ChangelogSource for RecChangelog {
    fn entries(&self) -> Vec<(String, String)> {
        vec![
            ("1.2.0".to_string(), "old".to_string()),
            ("1.1.0".to_string(), "older".to_string()),
        ]
    }
    fn new_entries(&self, last_version: &str) -> Vec<(String, String)> {
        self.entries()
            .into_iter()
            .filter(|(version, _)| version.as_str() > last_version)
            .collect()
    }
    fn normalize_links(&self, content: &str) -> String {
        content.to_string()
    }
}

fn key_display(_action: &str) -> String {
    // The harness keybindings resolve every action to `["ctrl+c"]`.
    "Ctrl+C".to_string()
}

/// The fixture assembly: recording collaborators plus the knob surface.
struct Fixture {
    view: Arc<RecView>,
    default_editor: Arc<RecEditor>,
    settings: Arc<RecSettings>,
    manager: Arc<RecSessionManager>,
    runtime: Arc<RecModelRuntime>,
    resources: Arc<RecResources>,
    shortcuts: Arc<RecShortcuts>,
    session: Arc<RecSession>,
    host: Arc<RecHost>,
    platform: Arc<RecPlatform>,
    /// `computeCacheWaste` / `getUsageCostBreakdown` projection.
    cache_stats: Option<(CacheWaste, Vec<UsageCostRow>)>,
}

impl Fixture {
    fn new(log: &Log) -> Self {
        let view = Arc::new(RecView::new(log.clone()));
        let default_editor = Arc::new(RecEditor::new(log.clone(), "defaultEditor"));
        let settings = Arc::new(RecSettings::new(log.clone()));
        let manager = Arc::new(RecSessionManager::new());
        let runtime = Arc::new(RecModelRuntime::new(log.clone()));
        let resources = Arc::new(RecResources::default());
        let shortcuts = Arc::new(RecShortcuts::new(log.clone()));
        let session = Arc::new(RecSession::new(
            log.clone(),
            runtime.clone(),
            resources.clone(),
            shortcuts.clone(),
        ));
        let host = Arc::new(RecHost::new(log.clone()));
        let platform = Arc::new(RecPlatform::new(log.clone()));
        Self {
            view,
            default_editor,
            settings,
            manager,
            runtime,
            resources,
            shortcuts,
            session,
            host,
            platform,
            cache_stats: None,
        }
    }

    fn build(&self, log: &Log, options: InteractiveModeOptions) -> Arc<InteractiveMode> {
        let io = ShellIo {
            session: self.session.clone(),
            session_manager: self.manager.clone(),
            settings: self.settings.clone(),
            view: self.view.clone(),
            host: self.host.clone(),
            commands: Arc::new(NullCommands),
            clock: Arc::new(FixedClock),
            platform: self.platform.clone(),
            default_editor: self.default_editor.clone(),
            default_model_per_provider: vec![
                ("anthropic".to_string(), "claude-opus-4-8".to_string()),
                ("radius".to_string(), "balanced".to_string()),
            ],
            auth_path: "/home/u/.pi/agent/auth.json".to_string(),
            docs_path: "/docs".to_string(),
            debug_log_path: "/tmp/pi-debug.log".to_string(),
            app_name: "pi".to_string(),
            app_title: "Pi".to_string(),
            version: "1.2.3".to_string(),
            home: "/home/u".to_string(),
            changelog: Box::new(RecChangelog),
            package_updates: Arc::new(FixturePackageUpdates::new(log.clone())),
            key_display: Box::new(key_display),
            theme: std::sync::RwLock::new(
                load_builtin_theme("dark", Some(ColorMode::Truecolor)).expect("built-in theme"),
            ),
            cache_stats: self.cache_stats.clone(),
            chalk_styler: Box::new(super::shell::default_chalk_dim),
        };
        Arc::new(InteractiveMode::new(io, options))
    }
}

struct FixedClock;

impl ShellClock for FixedClock {
    fn now_ms(&self) -> i64 {
        1_790_604_147_438
    }
}

struct NullCommands;

impl CommandSink for NullCommands {
    fn run(&self, _command: ShellCommand) {}
}

fn oracle() -> &'static Value {
    static ORACLE: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    ORACLE.get_or_init(|| {
        serde_json::from_str(include_str!(
            "../../../../tests/fixtures/interactive_r20_oracle/lower_oracle.json"
        ))
        .expect("lower oracle parses")
    })
}

fn oracle_log(scenario: &str) -> Vec<Value> {
    oracle()["scenarios"]
        .as_array()
        .expect("scenarios")
        .iter()
        .find(|s| s["name"] == json!(scenario))
        .unwrap_or_else(|| panic!("scenario {scenario} missing from oracle"))["log"]
        .as_array()
        .expect("log array")
        .clone()
}

/// Replays one scenario and asserts byte-parity with the node oracle.
fn replay_with(
    name: &str,
    options: InteractiveModeOptions,
    customize: impl FnOnce(&mut Fixture),
    drive: impl FnOnce(&Arc<InteractiveMode>, &Fixture, &Log) + Send,
) {
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let mut fixture = Fixture::new(&log);
    customize(&mut fixture);
    let shell = fixture.build(&log, options);
    shell.force_initialized();
    drive(&shell, &fixture, &log);
    let ours: Vec<Value> = log.lock().expect("log").clone();
    let expected = oracle_log(name);
    let failures: Vec<String> = ours
        .iter()
        .enumerate()
        .zip(expected.iter())
        .filter(|(ours_pair, expected)| ours_pair.1 != *expected)
        .map(|((index, actual), expected)| {
            format!(
                "  [{index}] ours:   {}\n      oracle: {}",
                serde_json::to_string(actual).unwrap_or_default(),
                serde_json::to_string(expected).unwrap_or_default()
            )
        })
        .collect();
    assert!(
        failures.is_empty() && ours.len() == expected.len(),
        "scenario {name} diverged ({} vs {} entries):\n{}",
        ours.len(),
        expected.len(),
        failures.join("\n")
    );
}

/// Replays one scenario against the default fixture.
fn replay(name: &str, drive: impl FnOnce(&Arc<InteractiveMode>, &Fixture, &Log) + Send) {
    replay_with(name, InteractiveModeOptions::default(), |_| {}, drive);
}

/// Resolves dialogs as they open, following a scripted `(component kind,
/// choice)` list. The close choreography runs on the awaiting shell task, so
/// the driver only pushes the choice through the pending dialog's resolve
/// channel; the kind filter keeps concurrent resolvers deterministic when a
/// ladder opens selector and editor dialogs interleaved.
fn resolve_when_open(
    shell: &Arc<InteractiveMode>,
    script: Vec<(&'static str, Option<String>)>,
) -> std::thread::JoinHandle<()> {
    let shell = shell.clone();
    std::thread::spawn(move || {
        for (kind, value) in script {
            let mut waited = 0usize;
            loop {
                let pending = shell
                    .lock()
                    .extension_dialog
                    .as_ref()
                    .map(|dialog| dialog.component.kind == kind)
                    .unwrap_or(false);
                if pending {
                    break;
                }
                waited += 1;
                assert!(waited < 10_000, "resolver timed out waiting for dialog");
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            shell.resolve_extension_dialog(value);
        }
    })
}

/// The selector-dialog kind the extension bridge mounts.
const SELECTOR_DIALOG: &str = "ExtensionSelectorComponent";
/// The editor-dialog kind the extension bridge mounts.
const EDITOR_DIALOG: &str = "ExtensionEditorComponent";
/// The input-dialog kind the extension bridge mounts.
const INPUT_DIALOG: &str = "ExtensionInputComponent";

/// Runs a `!`-returning shell method (the platform exit seam returns normally
/// in the fixture, so the shell's `unreachable!` tail panics — swallow it).
fn catching(drive: impl FnOnce()) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(drive));
}

// ---------------------------------------------------------------------------
// Scenario replays (byte-exact against lower_oracle.json)
// ---------------------------------------------------------------------------

mod lower_oracle {
    use super::*;

    // -- settings selector ----------------------------------------------------

    #[test]
    fn settings_open() {
        replay("settings.open", |shell, _, _| {
            shell.show_settings_selector();
        });
    }

    #[test]
    fn settings_cancel() {
        replay("settings.cancel", |shell, _, _| {
            shell.show_settings_selector();
            futures::executor::block_on(shell.settings_callback("onCancel", Value::Null));
        });
    }

    #[test]
    fn settings_auto_compact() {
        replay("settings.autoCompact", |shell, _, _| {
            shell.show_settings_selector();
            futures::executor::block_on(
                shell.settings_callback("onAutoCompactChange", json!(false)),
            );
            futures::executor::block_on(shell.settings_callback("onCancel", Value::Null));
        });
    }

    #[test]
    fn settings_hide_thinking() {
        replay("settings.hideThinking", |shell, _, _| {
            shell.show_settings_selector();
            futures::executor::block_on(
                shell.settings_callback("onHideThinkingBlockChange", json!(true)),
            );
            futures::executor::block_on(shell.settings_callback("onCancel", Value::Null));
        });
    }

    #[test]
    fn settings_output_pad_rebuild() {
        replay("settings.outputPad.rebuild", |shell, _, _| {
            shell.show_settings_selector();
            futures::executor::block_on(shell.settings_callback("onOutputPadChange", json!(3)));
            futures::executor::block_on(shell.settings_callback("onCancel", Value::Null));
        });
    }

    #[test]
    fn settings_tui_mode_conflict() {
        // The harness sets `hasOverlayEntries` on the `ui` handle while
        // `switchTuiMode` reads the renderer's flag, so the recorded output is
        // the switch-success path (the conflict branch stays oracle-unseen).
        replay("settings.tuiMode.conflict", |shell, _, _| {
            shell.show_settings_selector();
            futures::executor::block_on(
                shell.settings_callback("onTuiModeChange", json!("fullscreen")),
            );
            futures::executor::block_on(shell.settings_callback("onCancel", Value::Null));
        });
    }

    #[test]
    fn settings_tui_mode_ok() {
        replay("settings.tuiMode.ok", |shell, _, _| {
            shell.show_settings_selector();
            futures::executor::block_on(
                shell.settings_callback("onTuiModeChange", json!("fullscreen")),
            );
            futures::executor::block_on(shell.settings_callback("onCancel", Value::Null));
        });
    }

    #[test]
    fn settings_show_hardware_cursor() {
        replay("settings.showHardwareCursor", |shell, _, _| {
            shell.show_settings_selector();
            futures::executor::block_on(
                shell.settings_callback("onShowHardwareCursorChange", json!(true)),
            );
            futures::executor::block_on(
                shell.settings_callback("onClearOnShrinkChange", json!(false)),
            );
            futures::executor::block_on(shell.settings_callback("onCancel", Value::Null));
        });
    }

    // -- thinking ---------------------------------------------------------------

    #[test]
    fn thinking_command_unknown_level() {
        replay("thinking.command.unknown", |shell, _, _| {
            shell.handle_thinking_command(Some("bogus"));
        });
    }

    #[test]
    fn thinking_command_known_level() {
        replay("thinking.command.known", |shell, _, _| {
            shell.handle_thinking_command(Some("high"));
        });
    }

    #[test]
    fn thinking_command_bare() {
        replay("thinking.command.bare", |shell, _, _| {
            shell.handle_thinking_command(None);
        });
    }

    #[test]
    fn thinking_selector_persist() {
        replay("thinking.selector.persist", |shell, _, _| {
            shell.show_thinking_selector();
            shell.thinking_selector_callback("onSelect", Some(ThinkingLevel::Low));
        });
    }

    #[test]
    fn thinking_selector_persist_default() {
        replay("thinking.selector.persistDefault", |shell, _, _| {
            shell.show_thinking_selector();
            shell.thinking_selector_callback("onPersist", Some(ThinkingLevel::High));
        });
    }

    #[test]
    fn thinking_selector_cancel() {
        replay("thinking.selector.cancel", |shell, _, _| {
            shell.show_thinking_selector();
            shell.thinking_selector_callback("onCancel", None);
        });
    }

    // -- model --------------------------------------------------------------------

    #[test]
    fn model_command_exact_match() {
        replay("model.command.exact", |shell, _, _| {
            futures::executor::block_on(shell.handle_model_command(Some("openai/gpt-5.5")));
        });
    }

    #[test]
    fn model_command_exact_bare_id() {
        replay("model.command.exact.bare-id", |shell, _, _| {
            futures::executor::block_on(shell.handle_model_command(Some("gpt-5.5")));
        });
    }

    #[test]
    fn model_command_miss_falls_to_selector() {
        replay("model.command.miss.fallsToSelector", |shell, _, _| {
            futures::executor::block_on(shell.handle_model_command(Some("nope")));
        });
    }

    #[test]
    fn model_command_miss_refresh_then_selector() {
        replay_with(
            "model.command.miss.refresh.thenSelector",
            InteractiveModeOptions::default(),
            |fixture| {
                fixture.runtime.snapshot.lock().expect("snapshot").clear();
            },
            |shell, _, _| {
                futures::executor::block_on(shell.handle_model_command(Some("gpt-5.5")));
            },
        );
    }

    #[test]
    fn model_command_set_error() {
        replay_with(
            "model.command.setError",
            InteractiveModeOptions::default(),
            |fixture| {
                *fixture.session.set_model_error.lock().expect("error") =
                    Some("model rejected".to_string());
            },
            |shell, _, _| {
                futures::executor::block_on(shell.handle_model_command(Some("openai/gpt-5.5")));
            },
        );
    }

    #[test]
    fn model_selector_select() {
        replay("model.selector.select", |shell, fixture, _| {
            shell.show_model_selector(Some("gpt"));
            *fixture.session.set_model_log.lock().expect("override") = Some(json!(
                {"provider":"openai","id":"gpt-5.5","name":"GPT-5.5"}
            ));
            futures::executor::block_on(shell.model_selector_select(
                &ModelRef {
                    provider: "openai".to_string(),
                    id: "gpt-5.5".to_string(),
                    name: Some("GPT-5.5".to_string()),
                    api: None,
                    reasoning: false,
                },
                false,
            ));
        });
    }

    #[test]
    fn model_selector_persist() {
        replay("model.selector.persist", |shell, fixture, _| {
            shell.show_model_selector(None);
            *fixture.session.set_model_log.lock().expect("override") =
                Some(json!({"provider":"anthropic","id":"claude-opus-4-8"}));
            futures::executor::block_on(shell.model_selector_select(
                &ModelRef {
                    provider: "anthropic".to_string(),
                    id: "claude-opus-4-8".to_string(),
                    name: None,
                    api: None,
                    reasoning: false,
                },
                true,
            ));
        });
    }

    #[test]
    fn model_selector_cancel() {
        replay("model.selector.cancel", |shell, _, _| {
            shell.show_model_selector(None);
            shell.model_selector_cancel();
        });
    }

    #[test]
    fn model_selector_dispose() {
        replay("model.selector.dispose", |shell, fixture, _| {
            shell.show_model_selector(None);
            let component = shell.lock().model_selector.clone().expect("selector");
            // `selector.dispose()` — the component's own method.
            fixture
                .view
                .update_component(&component, "dispose", Value::Null);
            shell.dispose_active_selector();
        });
    }

    // -- scoped models ----------------------------------------------------------

    #[test]
    fn models_selector_toggle_and_persist() {
        replay("models.selector.toggleAndPersist", |shell, _, _| {
            futures::executor::block_on(shell.show_models_selector());
            futures::executor::block_on(
                shell.models_selector_change(Some(vec!["anthropic/claude-opus-4-8".to_string()])),
            );
            futures::executor::block_on(shell.models_selector_change(Some(vec![
                "anthropic/claude-opus-4-8".to_string(),
                "openai/gpt-5.5".to_string(),
            ])));
            futures::executor::block_on(shell.models_selector_change(None));
            shell.models_selector_persist(Some(vec!["anthropic/claude-opus-4-8".to_string()]));
            shell.models_selector_cancel();
        });
    }

    #[test]
    fn models_selector_persist_all() {
        replay("models.selector.persistAll", |shell, _, _| {
            futures::executor::block_on(shell.show_models_selector());
            shell.models_selector_persist(Some(vec![
                "anthropic/claude-opus-4-8".to_string(),
                "openai/gpt-5.5".to_string(),
            ]));
            shell.models_selector_persist(None);
            shell.models_selector_cancel();
        });
    }

    // -- fork / user message selector ----------------------------------------------

    #[test]
    fn fork_selector_empty() {
        replay("fork.empty", |shell, _, _| {
            shell.show_user_message_selector();
        });
    }

    #[test]
    fn fork_select() {
        replay("fork.select", |shell, fixture, _| {
            *fixture.session.user_messages.lock().expect("messages") = vec![
                ForkableUserMessage {
                    entry_id: "e1".to_string(),
                    text: "first".to_string(),
                },
                ForkableUserMessage {
                    entry_id: "e2".to_string(),
                    text: "second".to_string(),
                },
            ];
            shell.show_user_message_selector();
            futures::executor::block_on(shell.user_message_selector_select("e2"));
        });
    }

    #[test]
    fn fork_cancel() {
        replay("fork.cancel", |shell, fixture, _| {
            *fixture.session.user_messages.lock().expect("messages") = vec![ForkableUserMessage {
                entry_id: "e1".to_string(),
                text: "first".to_string(),
            }];
            shell.show_user_message_selector();
            shell.user_message_selector_cancel();
        });
    }

    #[test]
    fn clone_fresh() {
        replay("clone.fresh", |shell, _, _| {
            futures::executor::block_on(shell.handle_clone_command());
        });
    }

    #[test]
    fn clone_ok() {
        replay("clone.ok", |shell, _, _| {
            futures::executor::block_on(shell.handle_clone_command());
        });
    }

    // -- tree ------------------------------------------------------------------------

    #[test]
    fn tree_selector_empty() {
        replay("tree.empty", |shell, _, _| {
            shell.show_tree_selector(None);
        });
    }

    #[test]
    fn tree_leaf_noop() {
        replay_with(
            "tree.leaf.noop",
            InteractiveModeOptions::default(),
            |fixture| {
                *fixture.manager.tree.lock().expect("tree") =
                    vec![("leaf-1".to_string(), "message".to_string())];
            },
            |shell, _, _| {
                shell.show_tree_selector(None);
                let token = SelectorToken(
                    shell
                        .lock()
                        .tree_selector
                        .clone()
                        .expect("tree selector")
                        .id,
                );
                shell.selector_done(token);
                futures::executor::block_on(shell.tree_selector_select("leaf-1"));
            },
        );
    }

    #[test]
    fn tree_navigate_no_summary() {
        replay_with(
            "tree.navigate.noSummary",
            InteractiveModeOptions::default(),
            |fixture| {
                *fixture.manager.tree.lock().expect("tree") = vec![
                    ("leaf-1".to_string(), "message".to_string()),
                    ("e0".to_string(), "message".to_string()),
                ];
            },
            |shell, _, _| {
                shell.show_tree_selector(None);
                let token = SelectorToken(
                    shell
                        .lock()
                        .tree_selector
                        .clone()
                        .expect("tree selector")
                        .id,
                );
                shell.selector_done(token);
                let resolver = resolve_when_open(shell, vec![(SELECTOR_DIALOG, None)]);
                futures::executor::block_on(shell.tree_selector_select("e0"));
                resolver.join().expect("resolver");
            },
        );
    }

    #[test]
    fn tree_navigate_with_summary() {
        replay_with(
            "tree.navigate.withSummary",
            InteractiveModeOptions::default(),
            |fixture| {
                *fixture.manager.tree.lock().expect("tree") = vec![
                    ("leaf-1".to_string(), "message".to_string()),
                    ("e0".to_string(), "message".to_string()),
                ];
            },
            |shell, _, _| {
                shell.show_tree_selector(None);
                let token = SelectorToken(
                    shell
                        .lock()
                        .tree_selector
                        .clone()
                        .expect("tree selector")
                        .id,
                );
                shell.selector_done(token);
                let resolver = resolve_when_open(
                    shell,
                    vec![(SELECTOR_DIALOG, Some("Summarize".to_string()))],
                );
                futures::executor::block_on(shell.tree_selector_select("e0"));
                resolver.join().expect("resolver");
            },
        );
    }

    #[test]
    fn tree_navigate_custom_prompt_cancelled() {
        replay_with(
            "tree.navigate.customPromptCancelled",
            InteractiveModeOptions::default(),
            |fixture| {
                *fixture.manager.tree.lock().expect("tree") = vec![
                    ("leaf-1".to_string(), "message".to_string()),
                    ("e0".to_string(), "message".to_string()),
                ];
            },
            |shell, _, _| {
                shell.show_tree_selector(None);
                let token = SelectorToken(
                    shell
                        .lock()
                        .tree_selector
                        .clone()
                        .expect("tree selector")
                        .id,
                );
                shell.selector_done(token);
                // The ladder re-opens the summary selector after the editor
                // cancel; the kind filter keeps the two resolvers
                // deterministic.
                let resolver = resolve_when_open(
                    shell,
                    vec![
                        (
                            SELECTOR_DIALOG,
                            Some("Summarize with custom prompt".to_string()),
                        ),
                        (SELECTOR_DIALOG, Some("No summary".to_string())),
                    ],
                );
                let editor_cancel = resolve_when_open(shell, vec![(EDITOR_DIALOG, None)]);
                futures::executor::block_on(shell.tree_selector_select("e0"));
                resolver.join().expect("resolver");
                editor_cancel.join().expect("editor cancel");
            },
        );
    }

    #[test]
    fn tree_copy_no_text() {
        replay_with(
            "tree.copy.noText",
            InteractiveModeOptions::default(),
            |fixture| {
                *fixture.manager.tree.lock().expect("tree") =
                    vec![("leaf-1".to_string(), "message".to_string())];
            },
            |shell, _, _| {
                shell.show_tree_selector(None);
                futures::executor::block_on(shell.tree_selector_copy(""));
            },
        );
    }

    #[test]
    fn tree_copy_ok() {
        replay_with(
            "tree.copy.ok",
            InteractiveModeOptions::default(),
            |fixture| {
                *fixture.manager.tree.lock().expect("tree") =
                    vec![("leaf-1".to_string(), "message".to_string())];
            },
            |shell, _, _| {
                shell.show_tree_selector(None);
                futures::executor::block_on(shell.tree_selector_copy("entry text"));
            },
        );
    }

    // -- session selector / resume -----------------------------------------------------

    #[test]
    fn session_selector_open() {
        replay("session.selector.open", |shell, _, _| {
            shell.show_session_selector();
        });
    }

    #[test]
    fn resume_ok() {
        replay("resume.ok", |shell, _, _| {
            let _ = futures::executor::block_on(shell.handle_resume_session("/s/other.jsonl"));
        });
    }

    #[test]
    fn resume_missing_cwd_confirmed() {
        replay_with(
            "resume.missingCwd.confirmed",
            InteractiveModeOptions::default(),
            |fixture| {
                *fixture.host.switch_behavior.lock().expect("switch") =
                    SwitchBehavior::MissingCwd("/fallback".to_string());
            },
            |shell, _, log| {
                let resolver =
                    resolve_when_open(shell, vec![(SELECTOR_DIALOG, Some("Yes".to_string()))]);
                let result =
                    futures::executor::block_on(shell.handle_resume_session("/s/other.jsonl"));
                resolver.join().expect("resolver");
                rec(
                    log,
                    json!([
                        "resumeOutcome",
                        match result {
                            Ok(cancelled) => json!({ "result": { "cancelled": cancelled } }),
                            Err(error) => json!({ "error": error }),
                        }
                    ]),
                );
            },
        );
    }

    // -- trust ---------------------------------------------------------------------------

    #[test]
    fn trust_open() {
        replay("trust.open", |shell, _, _| {
            shell.show_trust_selector();
            shell.trust_selector_select(true, json!([{ "cwd": "/w", "trusted": true }]));
        });
    }

    #[test]
    fn trust_cancel() {
        replay("trust.cancel", |shell, _, _| {
            shell.show_trust_selector();
            shell.trust_selector_cancel();
        });
    }

    #[test]
    fn trust_auto_save_after_reload() {
        replay_with(
            "trust.autoSaveAfterReload",
            InteractiveModeOptions {
                auto_trust_on_reload_cwd: Some("/work/project".to_string()),
                ..InteractiveModeOptions::default()
            },
            |_| {},
            |shell, _, _| {
                let saved = shell.maybe_save_implicit_project_trust_after_reload();
                assert!(!saved);
            },
        );
    }

    // -- login ladders ---------------------------------------------------------------------

    #[test]
    fn login_bare() {
        replay("login.bare", |shell, _, _| {
            futures::executor::block_on(shell.handle_login_command(None));
        });
    }

    #[test]
    fn login_by_name_unique() {
        replay("login.byName.unique", |shell, _, _| {
            futures::executor::block_on(shell.handle_login_command(Some("OpenAI")));
        });
    }

    #[test]
    fn login_by_name_ambiguous() {
        replay("login.byName.ambiguous", |shell, fixture, _| {
            *fixture.runtime.providers.lock().expect("providers") = vec![
                json!({"id": "a1", "name": "Same", "auth": {"apiKey": true}}),
                json!({"id": "a2", "name": "Same", "auth": {"apiKey": true}}),
            ];
            futures::executor::block_on(shell.handle_login_command(Some("same")));
        });
    }

    #[test]
    fn login_by_name_unknown() {
        replay("login.byName.unknown", |shell, _, _| {
            futures::executor::block_on(shell.handle_login_command(Some("mystery")));
        });
    }

    #[test]
    fn login_auth_type_oauth() {
        replay("login.authType.oauth", |shell, _, _| {
            shell.show_login_auth_type_selector(None);
            futures::executor::block_on(shell.login_auth_type_select("Sign in with an account"));
        });
    }

    #[test]
    fn login_provider_oauth() {
        replay("login.provider.oauth", |shell, _, _| {
            shell.show_login_provider_selector(Some("oauth"), None);
            futures::executor::block_on(shell.login_provider_select("anthropic", "oauth"));
        });
    }

    #[test]
    fn login_provider_empty() {
        replay("login.provider.empty", |shell, fixture, _| {
            fixture.runtime.providers.lock().expect("providers").clear();
            shell.show_login_provider_selector(Some("oauth"), None);
            shell.show_login_provider_selector(Some("api_key"), None);
            shell.show_login_provider_selector(None, None);
        });
    }

    #[test]
    fn login_oauth_dialog() {
        replay("login.oauthDialog", |shell, _, _| {
            futures::executor::block_on(shell.show_login_dialog("anthropic", "Anthropic"));
        });
    }

    #[test]
    fn login_api_key_dialog() {
        replay("login.apiKeyDialog", |shell, _, _| {
            futures::executor::block_on(shell.show_api_key_login_dialog("openai", "OpenAI"));
        });
    }

    #[test]
    fn login_ambient() {
        replay("login.ambient", |shell, _, _| {
            shell.show_ambient_auth_dialog(&AuthProviderOption {
                id: "ollama".to_string(),
                name: "Ollama".to_string(),
                auth_type: "api_key".to_string(),
                method_login: true,
                method_name: None,
                login_label: None,
                status: None,
                subscription: None,
            });
        });
    }

    #[test]
    fn login_bedrock_details() {
        replay("login.bedrockDetails", |shell, _, _| {
            futures::executor::block_on(
                shell.show_api_key_login_dialog("amazon-bedrock", "Bedrock"),
            );
        });
    }

    #[test]
    fn login_auth_select() {
        replay("login.authSelect", |shell, _, _| {
            let dialog = ComponentRef {
                kind: "LoginDialogComponent".to_string(),
                id: 0,
            };
            let resolver = resolve_when_open(shell, vec![(SELECTOR_DIALOG, Some("B".to_string()))]);
            let result = futures::executor::block_on(shell.show_auth_select(
                &dialog,
                &json!({
                    "type": "select",
                    "message": "Pick one",
                    "options": [{"id": "a", "label": "A"}, {"id": "b", "label": "B"}],
                }),
                "anthropic",
            ));
            resolver.join().expect("resolver");
            assert_eq!(result, Ok("b".to_string()));
        });
    }

    #[test]
    fn login_auth_select_cancelled() {
        replay("login.authSelect.cancelled", |shell, _, log| {
            let dialog = ComponentRef {
                kind: "LoginDialogComponent".to_string(),
                id: 0,
            };
            let resolver = resolve_when_open(shell, vec![(SELECTOR_DIALOG, None)]);
            let result = futures::executor::block_on(shell.show_auth_select(
                &dialog,
                &json!({
                    "type": "select",
                    "message": "Pick one",
                    "options": [{"id": "a", "label": "A"}],
                }),
                "anthropic",
            ));
            resolver.join().expect("resolver");
            if let Err(error) = result {
                rec(log, json!(["caught", error]));
            }
        });
    }

    #[test]
    fn login_auth_prompt_select() {
        replay("login.authPrompt.select", |shell, _, _| {
            let dialog = ComponentRef {
                kind: "LoginDialogComponent".to_string(),
                id: 0,
            };
            let resolver = resolve_when_open(shell, vec![(SELECTOR_DIALOG, Some("A".to_string()))]);
            let result = futures::executor::block_on(shell.show_auth_prompt(
                &dialog,
                &json!({
                    "type": "select",
                    "message": "Pick one",
                    "options": [{"id": "a", "label": "A"}],
                }),
                "anthropic",
            ));
            resolver.join().expect("resolver");
            assert_eq!(result, Ok("a".to_string()));
        });
    }

    #[test]
    fn login_auth_prompt_manual() {
        replay("login.authPrompt.manual", |shell, _, _| {
            let dialog = ComponentRef {
                kind: "LoginDialogComponent".to_string(),
                id: 0,
            };
            let result = futures::executor::block_on(shell.show_auth_prompt(
                &dialog,
                &json!({ "type": "manual_code", "message": "Enter code" }),
                "anthropic",
            ));
            assert!(result.is_ok());
        });
    }

    #[test]
    fn login_auth_prompt_signal_aborted() {
        replay("login.authPrompt.signalAborted", |shell, _, log| {
            let dialog = ComponentRef {
                kind: "LoginDialogComponent".to_string(),
                id: 0,
            };
            let result = futures::executor::block_on(shell.show_auth_prompt(
                &dialog,
                &json!({
                    "type": "manual_code",
                    "message": "Enter code",
                    "signal": { "aborted": true },
                }),
                "anthropic",
            ));
            if let Err(error) = result {
                rec(log, json!(["caught", error]));
            }
        });
    }

    #[test]
    fn login_notify_device_code() {
        replay("login.notify.deviceCode", |shell, _, _| {
            let dialog = ComponentRef {
                kind: "LoginDialogComponent".to_string(),
                id: 0,
            };
            shell.notify_auth_dialog(
                &dialog,
                &json!({
                    "type": "device_code",
                    "verificationUrl": "https://example/activate",
                    "userCode": "ABC-123",
                    "websiteUrl": null,
                    "expiresInSeconds": 600,
                    "interval": 5,
                    "providerId": "x",
                    "message": null,
                }),
            );
            shell.notify_auth_dialog(
                &dialog,
                &json!({ "type": "info", "message": "hello", "links": [] }),
            );
            shell.notify_auth_dialog(
                &dialog,
                &json!({ "type": "progress", "message": "working" }),
            );
            shell.notify_auth_dialog(
                &dialog,
                &json!({ "type": "auth_url", "url": "https://x", "instructions": "go" }),
            );
        });
    }

    /// The scenario's default `this` carries the harness `fakeModelRuntime`,
    /// which has no `getProvider` member: `getLogoutProviderOptions` throws
    /// `this.session.modelRuntime.getProvider is not a function` right after
    /// the logged `listCredentials` call, and `showOAuthSelector`'s catch
    /// renders the error. The fixture knob reproduces the stub shape.
    #[test]
    fn logout_selector() {
        replay("logout.selector", |shell, fixture, _| {
            fixture
                .runtime
                .get_provider_missing
                .store(true, Ordering::SeqCst);
            futures::executor::block_on(shell.show_oauth_selector("logout"));
        });
    }

    #[test]
    fn logout_empty() {
        replay_with(
            "logout.empty",
            InteractiveModeOptions::default(),
            |fixture| {
                *fixture.runtime.credentials.lock().expect("credentials") = Ok(Vec::new());
                fixture
                    .runtime
                    .credentials_logged
                    .store(false, Ordering::SeqCst);
            },
            |shell, _, _| {
                futures::executor::block_on(shell.show_oauth_selector("logout"));
            },
        );
    }

    #[test]
    fn logout_error() {
        replay_with(
            "logout.error",
            InteractiveModeOptions::default(),
            |fixture| {
                *fixture.runtime.credentials.lock().expect("credentials") =
                    Err("keychain locked".to_string());
                fixture
                    .runtime
                    .credentials_logged
                    .store(false, Ordering::SeqCst);
            },
            |shell, _, _| {
                futures::executor::block_on(shell.show_oauth_selector("logout"));
            },
        );
    }

    // -- completion of provider authentication --------------------------------------------

    #[test]
    fn complete_auth_known_model() {
        replay("completeAuth.knownModel", |shell, _, _| {
            futures::executor::block_on(shell.complete_provider_authentication(
                "openai",
                "OpenAI",
                "api_key",
                Some(&ModelRef {
                    provider: "openai".to_string(),
                    id: "gpt-5.5".to_string(),
                    name: None,
                    api: Some("openai".to_string()),
                    reasoning: false,
                }),
            ));
        });
    }

    #[test]
    fn complete_auth_unknown_model_default_available() {
        replay(
            "completeAuth.unknownModel.defaultAvailable",
            |shell, _, _| {
                futures::executor::block_on(shell.complete_provider_authentication(
                    "anthropic",
                    "Anthropic",
                    "oauth",
                    Some(&ModelRef {
                        provider: "unknown".to_string(),
                        id: "unknown".to_string(),
                        name: None,
                        api: Some("unknown".to_string()),
                        reasoning: false,
                    }),
                ));
            },
        );
    }

    #[test]
    fn complete_auth_unknown_model_deferred() {
        replay("completeAuth.unknownModel.deferred", |shell, fixture, _| {
            fixture.runtime.snapshot.lock().expect("snapshot").clear();
            let previous = unknown_model();
            *fixture.session.model.lock().expect("model") = Some(previous.clone());
            futures::executor::block_on(shell.complete_provider_authentication(
                "radius",
                "Radius",
                "oauth",
                Some(&previous),
            ));
        });
    }

    #[test]
    fn complete_auth_llama() {
        replay("completeAuth.llama", |shell, fixture, _| {
            fixture.runtime.snapshot.lock().expect("snapshot").clear();
            let previous = unknown_model();
            *fixture.session.model.lock().expect("model") = Some(previous.clone());
            futures::executor::block_on(shell.complete_provider_authentication(
                "llama.cpp",
                "llama.cpp",
                "api_key",
                Some(&previous),
            ));
        });
    }

    #[test]
    fn complete_auth_no_default_provider() {
        replay("completeAuth.noDefaultProvider", |shell, fixture, _| {
            fixture.runtime.snapshot.lock().expect("snapshot").clear();
            let previous = unknown_model();
            *fixture.session.model.lock().expect("model") = Some(previous.clone());
            futures::executor::block_on(shell.complete_provider_authentication(
                "mystery-provider",
                "Mystery",
                "api_key",
                Some(&previous),
            ));
        });
    }

    fn unknown_model() -> ModelRef {
        ModelRef {
            provider: "unknown".to_string(),
            id: "unknown".to_string(),
            name: None,
            api: Some("unknown".to_string()),
            reasoning: false,
        }
    }

    fn anthropic_model() -> ModelRef {
        ModelRef {
            provider: "anthropic".to_string(),
            id: "claude-opus-4-8".to_string(),
            name: None,
            api: None,
            reasoning: false,
        }
    }

    // -- anthropic subscription warning ------------------------------------------------------

    #[test]
    fn warn_anthropic_disabled() {
        replay_with(
            "warn.anthropic.disabled",
            InteractiveModeOptions::default(),
            |fixture| {
                fixture
                    .settings
                    .warnings_anthropic
                    .store(false, Ordering::SeqCst);
            },
            |shell, _, _| {
                futures::executor::block_on(
                    shell.maybe_warn_about_anthropic_subscription_auth(Some(&anthropic_model())),
                );
            },
        );
    }

    #[test]
    fn warn_anthropic_oauth() {
        replay_with(
            "warn.anthropic.oauth",
            InteractiveModeOptions::default(),
            |fixture| {
                *fixture.runtime.check_auth.lock().expect("check") = Some("oauth".to_string());
                fixture
                    .runtime
                    .auth_calls_logged
                    .store(false, Ordering::SeqCst);
            },
            |shell, _, _| {
                futures::executor::block_on(
                    shell.maybe_warn_about_anthropic_subscription_auth(Some(&anthropic_model())),
                );
            },
        );
    }

    #[test]
    fn warn_anthropic_oat_key() {
        replay("warn.anthropic.oatKey", |shell, _, _| {
            futures::executor::block_on(
                shell.maybe_warn_about_anthropic_subscription_auth(Some(&anthropic_model())),
            );
        });
    }

    #[test]
    fn warn_anthropic_plain_key() {
        replay_with(
            "warn.anthropic.plainKey",
            InteractiveModeOptions::default(),
            |fixture| {
                *fixture.runtime.auth_key.lock().expect("key") = Some("sk-plain".to_string());
                *fixture.runtime.check_auth.lock().expect("check") = Some("api_key".to_string());
                fixture
                    .runtime
                    .auth_calls_logged
                    .store(false, Ordering::SeqCst);
            },
            |shell, _, _| {
                futures::executor::block_on(
                    shell.maybe_warn_about_anthropic_subscription_auth(Some(&anthropic_model())),
                );
            },
        );
    }

    #[test]
    fn warn_anthropic_non_anthropic() {
        replay("warn.anthropic.nonAnthropic", |shell, _, _| {
            futures::executor::block_on(shell.maybe_warn_about_anthropic_subscription_auth(Some(
                &ModelRef {
                    provider: "openai".to_string(),
                    id: "gpt-5.5".to_string(),
                    name: None,
                    api: None,
                    reasoning: false,
                },
            )));
        });
    }

    // -- command handlers ---------------------------------------------------------------------

    #[test]
    fn name_command_set() {
        replay("cmd.name.set", |shell, _, _| {
            shell.handle_name_command("/name My Session");
        });
    }

    #[test]
    fn name_command_empty() {
        replay("cmd.name.empty", |shell, _, _| {
            shell.handle_name_command("/name");
        });
    }

    #[test]
    fn name_command_existing() {
        replay_with(
            "cmd.name.existing",
            InteractiveModeOptions::default(),
            |fixture| {
                *fixture.manager.session_name.lock().expect("name") = Some("existing".to_string());
            },
            |shell, _, _| {
                shell.handle_name_command("/name");
            },
        );
    }

    #[test]
    fn name_command_normalized() {
        replay_with(
            "cmd.name.normalized",
            InteractiveModeOptions::default(),
            |fixture| {
                *fixture.manager.session_name.lock().expect("name") =
                    Some("normalized".to_string());
            },
            |shell, _, _| {
                shell.handle_name_command("/name raw name");
            },
        );
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn session_command() {
        replay_with(
            "cmd.session",
            InteractiveModeOptions::default(),
            |fixture| {
                let mut stats = SessionStats::default();
                stats.session_file = Some("/s.jsonl".to_string());
                stats.session_id = "abc123".to_string();
                stats.total_messages = 4;
                stats.user_messages = 2;
                stats.assistant_messages = 1;
                stats.tool_calls = 1;
                stats.tool_results = 1;
                stats.tokens_input = 1200;
                stats.tokens_output = 34;
                stats.tokens_cache_read = 0;
                stats.tokens_cache_write = 100;
                stats.tokens_total = 1334;
                stats.cost = 0.0123;
                *fixture.session.stats.lock().expect("stats") = stats;
                fixture.cache_stats = Some((
                    CacheWaste {
                        missed_tokens: 25000,
                        missed_cost: 0.02,
                        miss_count: 2,
                    },
                    vec![
                        UsageCostRow {
                            key: "anthropic/claude-opus-4-8".to_string(),
                            cost: 0.01,
                            tokens: 1000,
                        },
                        UsageCostRow {
                            key: "openai/gpt-5.5".to_string(),
                            cost: 0.0023,
                            tokens: 334,
                        },
                    ],
                ));
            },
            |shell, _, _| {
                shell.handle_session_command();
            },
        );
    }

    #[test]
    fn changelog_command() {
        replay("cmd.changelog", |shell, _, _| {
            shell.handle_changelog_command();
        });
    }

    #[test]
    fn hotkeys_command() {
        replay("cmd.hotkeys", |shell, _, _| {
            shell.handle_hotkeys_command();
        });
    }

    #[test]
    fn debug_command() {
        replay("cmd.debug", |shell, _, _| {
            shell.handle_debug_command();
        });
    }

    #[test]
    fn path_argument_grid() {
        replay("cmd.pathArg.grid", |shell, _, log| {
            let cases = [
                ("/export", "/export"),
                ("/export", "/export out.jsonl"),
                ("/export", "/export 'my file.jsonl'"),
                ("/export", "/export \"double.jsonl\""),
                ("/export", "/export unterminated'"),
                ("/export", "/export a b c"),
                ("/export", "/export "),
                ("/export", "/exportx y"),
                ("/import", "/import in.jsonl"),
            ];
            for (command, text) in cases {
                rec(
                    log,
                    json!([
                        "pathArg",
                        text,
                        shell
                            .get_path_command_argument(text, command)
                            .map(Value::from)
                            .unwrap_or(Value::Null)
                    ]),
                );
            }
        });
    }

    #[test]
    fn export_command_jsonl() {
        replay("cmd.export.jsonl", |shell, _, _| {
            futures::executor::block_on(shell.handle_export_command("/export out.jsonl"));
        });
    }

    #[test]
    fn export_command_html() {
        replay("cmd.export.html", |shell, _, _| {
            futures::executor::block_on(shell.handle_export_command("/export"));
        });
    }

    #[test]
    fn export_command_error() {
        replay_with(
            "cmd.export.error",
            InteractiveModeOptions::default(),
            |fixture| {
                *fixture.session.export_jsonl_error.lock().expect("export") =
                    Some("disk full".to_string());
            },
            |shell, _, _| {
                futures::executor::block_on(shell.handle_export_command("/export out.jsonl"));
            },
        );
    }

    #[test]
    fn import_command_confirmed() {
        replay("cmd.import.confirmed", |shell, _, _| {
            let resolver =
                resolve_when_open(shell, vec![(SELECTOR_DIALOG, Some("Yes".to_string()))]);
            futures::executor::block_on(shell.handle_import_command("/import in.jsonl"));
            resolver.join().expect("resolver");
        });
    }

    #[test]
    fn import_command_declined() {
        replay("cmd.import.declined", |shell, _, _| {
            let resolver =
                resolve_when_open(shell, vec![(SELECTOR_DIALOG, Some("No".to_string()))]);
            futures::executor::block_on(shell.handle_import_command("/import in.jsonl"));
            resolver.join().expect("resolver");
        });
    }

    #[test]
    fn import_command_missing_cwd() {
        replay_with(
            "cmd.import.missingCwd",
            InteractiveModeOptions::default(),
            |fixture| {
                *fixture.host.import_behavior.lock().expect("import") =
                    ImportBehavior::MissingCwd("/fallback".to_string());
            },
            |shell, _, log| {
                let resolver = resolve_when_open(
                    shell,
                    vec![
                        (SELECTOR_DIALOG, Some("Yes".to_string())),
                        (SELECTOR_DIALOG, Some("Yes".to_string())),
                    ],
                );
                futures::executor::block_on(shell.handle_import_command("/import in.jsonl"));
                resolver.join().expect("resolver");
                rec(log, json!(["importOutcome", "done"]));
            },
        );
    }

    #[test]
    fn import_command_file_not_found() {
        replay_with(
            "cmd.import.fileNotFound",
            InteractiveModeOptions::default(),
            |fixture| {
                *fixture.host.import_behavior.lock().expect("import") =
                    ImportBehavior::NotFound("file not found: in.jsonl".to_string());
            },
            |shell, _, _| {
                let resolver =
                    resolve_when_open(shell, vec![(SELECTOR_DIALOG, Some("Yes".to_string()))]);
                futures::executor::block_on(shell.handle_import_command("/import in.jsonl"));
                resolver.join().expect("resolver");
            },
        );
    }

    #[test]
    fn share_command() {
        replay("cmd.share", |shell, _, _| {
            futures::executor::block_on(shell.handle_share_command());
        });
    }

    #[test]
    fn copy_command_plain() {
        replay("cmd.copy.plain", |shell, _, _| {
            futures::executor::block_on(shell.handle_copy_command(false, false));
        });
    }

    #[test]
    fn copy_command_flash() {
        replay("cmd.copy.flash", |shell, _, _| {
            futures::executor::block_on(shell.handle_copy_command(true, true));
        });
    }

    #[test]
    fn copy_command_none() {
        replay_with(
            "cmd.copy.none",
            InteractiveModeOptions::default(),
            |fixture| {
                *fixture.session.last_assistant_text.lock().expect("last") = None;
            },
            |shell, _, _| {
                futures::executor::block_on(shell.handle_copy_command(false, false));
            },
        );
    }

    #[test]
    fn clear_command() {
        replay("cmd.clear", |shell, _, _| {
            futures::executor::block_on(shell.handle_clear_command());
        });
    }

    /// The scenario stubs `runtimeHost.newSession` wholesale to
    /// `{ cancelled: true }`; `handleClearCommand` returns after the status
    /// clear with no new-session chatter.
    #[test]
    fn clear_command_cancelled() {
        replay_with(
            "cmd.clear.cancelled",
            InteractiveModeOptions::default(),
            |fixture| {
                fixture
                    .host
                    .new_session_cancelled
                    .store(true, Ordering::SeqCst);
            },
            |shell, _, _| {
                futures::executor::block_on(shell.handle_clear_command());
            },
        );
    }

    #[test]
    fn compact_command() {
        replay("cmd.compact", |shell, _, _| {
            futures::executor::block_on(shell.handle_compact_command(Some("focus on tests")));
        });
    }

    #[test]
    fn bash_command_extension_result() {
        replay_with(
            "cmd.bash.extensionResult",
            InteractiveModeOptions::default(),
            |fixture| {
                *fixture.shortcuts.user_bash.lock().expect("bash") =
                    ShortcutsUserBash::FullResult(BashOutcome {
                        exit_code: Some(0),
                        cancelled: false,
                        output: "hi".to_string(),
                        truncated: false,
                        full_output_path: None,
                    });
            },
            |shell, _, _| {
                futures::executor::block_on(shell.handle_bash_command("echo hi", false));
            },
        );
    }

    #[test]
    fn bash_command_exec() {
        replay("cmd.bash.exec", |shell, _, _| {
            futures::executor::block_on(shell.handle_bash_command("echo hi", false));
        });
    }

    #[test]
    fn bash_command_exec_streaming_deferred() {
        replay_with(
            "cmd.bash.exec.streamingDeferred",
            InteractiveModeOptions::default(),
            |fixture| {
                fixture.session.streaming.store(true, Ordering::SeqCst);
            },
            |shell, _, _| {
                futures::executor::block_on(shell.handle_bash_command("echo hi", false));
            },
        );
    }

    #[test]
    fn bash_command_exec_error() {
        replay_with(
            "cmd.bash.exec.error",
            InteractiveModeOptions::default(),
            |fixture| {
                *fixture.session.execute_bash.lock().expect("bash") =
                    Err("spawn failed".to_string());
            },
            |shell, _, _| {
                futures::executor::block_on(shell.handle_bash_command("echo hi", false));
            },
        );
    }

    #[test]
    fn bash_command_emit_throws() {
        replay_with(
            "cmd.bash.emitThrows",
            InteractiveModeOptions::default(),
            |fixture| {
                *fixture.shortcuts.user_bash.lock().expect("bash") = ShortcutsUserBash::Throws;
            },
            |shell, _, _| {
                futures::executor::block_on(shell.handle_bash_command("echo hi", false));
            },
        );
    }

    // v1.0.0 removed the daxnuts easter egg (component, `handleDaxnuts`, and
    // `checkDaxnutsEasterEgg`); the `easter.daxnuts` recording in the r20
    // fixture predates it and its replay went with the code. The armin and
    // demented-delves handlers it also drove are covered by their component
    // oracles.

    // -- reload -------------------------------------------------------------------------

    #[test]
    fn reload_blocked_streaming() {
        replay_with(
            "reload.blocked.streaming",
            InteractiveModeOptions::default(),
            |fixture| {
                fixture.session.streaming.store(true, Ordering::SeqCst);
            },
            |shell, _, _| {
                futures::executor::block_on(shell.handle_reload_command());
            },
        );
    }

    #[test]
    fn reload_blocked_compacting() {
        replay_with(
            "reload.blocked.compacting",
            InteractiveModeOptions::default(),
            |fixture| {
                fixture.session.compacting.store(true, Ordering::SeqCst);
            },
            |shell, _, _| {
                futures::executor::block_on(shell.handle_reload_command());
            },
        );
    }

    #[test]
    fn reload_ok() {
        replay("reload.ok", |shell, _, _| {
            futures::executor::block_on(shell.handle_reload_command());
        });
    }

    #[test]
    fn reload_failure() {
        replay_with(
            "reload.failure",
            InteractiveModeOptions::default(),
            |fixture| {
                *fixture.session.reload.lock().expect("reload") = Err("boom".to_string());
            },
            |shell, _, _| {
                futures::executor::block_on(shell.handle_reload_command());
            },
        );
    }

    // -- extension ui dialogs --------------------------------------------------------------

    #[test]
    fn extui_selector_choose() {
        replay("extui.selector.choose", |shell, _, _| {
            let resolver = resolve_when_open(shell, vec![(SELECTOR_DIALOG, Some("B".to_string()))]);
            let choice = futures::executor::block_on(shell.extension_selector_choice(
                "Pick",
                &["A".to_string(), "B".to_string()],
                None,
            ));
            resolver.join().expect("resolver");
            assert_eq!(choice, Some("B".to_string()));
        });
    }

    #[test]
    fn extui_selector_cancel() {
        replay("extui.selector.cancel", |shell, _, _| {
            let resolver = resolve_when_open(shell, vec![(SELECTOR_DIALOG, None)]);
            let choice = futures::executor::block_on(shell.extension_selector_choice(
                "Pick",
                &["A".to_string()],
                None,
            ));
            resolver.join().expect("resolver");
            assert_eq!(choice, None);
        });
    }

    #[test]
    fn extui_selector_aborted_signal() {
        replay("extui.selector.abortedSignal", |shell, _, _| {
            // Pre-arm an aborted dialog: the choice returns without mounting.
            let (tx, _rx) = tokio::sync::oneshot::channel::<Option<String>>();
            shell.lock().extension_dialog = Some(ExtensionDialog {
                component: ComponentRef {
                    kind: "ExtensionSelectorComponent".to_string(),
                    id: 0,
                },
                aborted: true,
                resolve: tx,
            });
            let choice = futures::executor::block_on(shell.extension_selector_choice(
                "Pick",
                &["A".to_string()],
                None,
            ));
            assert_eq!(choice, None);
        });
    }

    #[test]
    fn extui_confirm() {
        replay("extui.confirm", |shell, _, _| {
            let resolver =
                resolve_when_open(shell, vec![(SELECTOR_DIALOG, Some("Yes".to_string()))]);
            let confirmed =
                futures::executor::block_on(shell.show_extension_confirm("Title", "Body"));
            resolver.join().expect("resolver");
            assert!(confirmed);
        });
    }

    #[test]
    fn extui_confirm_no() {
        replay("extui.confirm.no", |shell, _, _| {
            let resolver =
                resolve_when_open(shell, vec![(SELECTOR_DIALOG, Some("No".to_string()))]);
            let confirmed =
                futures::executor::block_on(shell.show_extension_confirm("Title", "Body"));
            resolver.join().expect("resolver");
            assert!(!confirmed);
        });
    }

    #[test]
    fn extui_input() {
        replay("extui.input", |shell, _, _| {
            let resolver =
                resolve_when_open(shell, vec![(INPUT_DIALOG, Some("typed".to_string()))]);
            let value =
                futures::executor::block_on(shell.show_extension_input("Name", Some("hint")));
            resolver.join().expect("resolver");
            assert_eq!(value, Some("typed".to_string()));
        });
    }

    #[test]
    fn extui_editor() {
        replay("extui.editor", |shell, _, _| {
            let resolver = resolve_when_open(
                shell,
                vec![(EDITOR_DIALOG, Some("edited text".to_string()))],
            );
            let value =
                futures::executor::block_on(shell.show_extension_editor("Edit", Some("seed")));
            resolver.join().expect("resolver");
            assert_eq!(value, Some("edited text".to_string()));
        });
    }

    #[test]
    fn extui_notify_levels() {
        replay("extui.notify", |shell, _, _| {
            shell.show_extension_notify("info message", None);
            shell.show_extension_notify("warn message", Some("warning"));
            shell.show_extension_notify("error message", Some("error"));
        });
    }

    #[test]
    fn extui_error_stack() {
        replay("extui.error.stack", |shell, _, _| {
            shell.show_extension_error(
                "/ext/path.ts",
                "exploded",
                Some("Error: exploded\n    at fn (/ext/path.ts:1:1)\n    at inner (/x:2:2)"),
            );
        });
    }

    #[test]
    fn extui_custom_editor_mode() {
        replay("extui.custom.editorMode", |shell, _, _| {
            let resolver = resolve_when_open(
                shell,
                vec![("CustomExt", Some("custom-result".to_string()))],
            );
            let result = futures::executor::block_on(shell.show_extension_custom(false));
            resolver.join().expect("resolver");
            assert_eq!(result, Some("custom-result".to_string()));
        });
    }

    #[test]
    fn extui_custom_overlay() {
        replay("extui.custom.overlay", |shell, _, _| {
            let resolver = resolve_when_open(
                shell,
                vec![("CustomExt", Some("overlay-result".to_string()))],
            );
            let result = futures::executor::block_on(shell.show_extension_custom(true));
            resolver.join().expect("resolver");
            assert_eq!(result, Some("overlay-result".to_string()));
        });
    }

    #[test]
    fn extui_custom_editor_swap() {
        replay("extui.customEditor.swap", |shell, fixture, log| {
            let custom: Arc<dyn ShellEditor> =
                Arc::new(RecEditor::new(log.clone(), "customEditor"));
            fixture.view.editor_custom.store(true, Ordering::SeqCst);
            shell.set_custom_editor_component(Some(custom));
            fixture.view.editor_custom.store(false, Ordering::SeqCst);
            shell.set_custom_editor_component(None);
        });
    }

    #[test]
    fn extui_reset_extension_ui() {
        replay("extui.resetExtensionUI", |shell, _, _| {
            shell.reset_extension_ui();
        });
    }

    // -- shortcuts -------------------------------------------------------------------------

    #[test]
    fn shortcuts_setup_and_match() {
        replay("shortcuts.setupAndMatch", |shell, fixture, log| {
            *fixture.shortcuts.shortcuts.lock().expect("shortcuts") = vec![ExtensionShortcut {
                key: "ctrl+shift+g".to_string(),
                description: Some("go".to_string()),
                extension_path: String::new(),
            }];
            shell.setup_extension_shortcuts();
            // The extension handler body records its own invocation.
            rec(log, json!(["shortcut.handler"]));
            let matched = shell.on_extension_shortcut("ctrl+shift+g");
            rec(log, json!(["shortcut.match", matched]));
            let missed = shell.on_extension_shortcut("ctrl+shift+z");
            rec(log, json!(["shortcut.miss", missed]));
        });
    }

    // -- loaded resources --------------------------------------------------------------------

    #[test]
    fn loaded_resources_quiet_skip() {
        replay("resources.quietSkip", |shell, _, _| {
            shell.show_loaded_resources(false, false);
        });
    }

    #[test]
    fn loaded_resources_full() {
        replay_with(
            "resources.full",
            InteractiveModeOptions::default(),
            |fixture| {
                let resource = |path: &str| LoadedResource {
                    name: None,
                    path: path.to_string(),
                    source_info: None,
                    source_path: None,
                    hidden: false,
                };
                *fixture.resources.skills.lock().expect("skills") = ResourceGroupRead {
                    items: vec![LoadedResource {
                        name: Some("search".to_string()),
                        path: "C:\\work\\.pi\\skills\\search\\SKILL.md".to_string(),
                        source_info: None,
                        source_path: None,
                        hidden: false,
                    }],
                    diagnostics: Vec::new(),
                };
                *fixture.resources.prompts.lock().expect("prompts") = ResourceGroupRead {
                    items: vec![LoadedResource {
                        name: Some("review".to_string()),
                        path: "C:\\work\\.pi\\prompts\\review.md".to_string(),
                        source_info: None,
                        source_path: None,
                        hidden: false,
                    }],
                    diagnostics: Vec::new(),
                };
                *fixture.resources.themes.lock().expect("themes") = ResourceGroupRead {
                    items: vec![LoadedResource {
                        name: Some("solarized".to_string()),
                        path: String::new(),
                        source_info: None,
                        source_path: Some("C:\\work\\.pi\\themes\\solarized.json".to_string()),
                        hidden: false,
                    }],
                    diagnostics: Vec::new(),
                };
                *fixture.resources.extensions.lock().expect("extensions") = (
                    vec![LoadedResource {
                        name: None,
                        path: "C:\\work\\.pi\\extensions\\tag.ts".to_string(),
                        source_info: None,
                        source_path: None,
                        hidden: false,
                    }],
                    Vec::new(),
                );
                *fixture.resources.system_prompt_source.lock().expect("sys") =
                    Some(resource("C:\\work\\PI.md"));
                *fixture.resources.append_sources.lock().expect("append") =
                    vec![resource("C:\\work\\EXTRA.md")];
                *fixture.resources.agents_files.lock().expect("agents") =
                    vec![resource("C:\\work\\AGENTS.md")];
            },
            |shell, _, _| {
                shell.show_loaded_resources(true, false);
            },
        );
    }

    #[test]
    fn loaded_resources_diagnostics() {
        replay_with(
            "resources.diagnostics",
            InteractiveModeOptions::default(),
            |fixture| {
                use super::super::interactive_mode::DiagnosticKind;
                let diagnostic = |kind, message: &str, path: &str| ResourceDiagnostic {
                    kind,
                    message: message.to_string(),
                    path: Some(path.to_string()),
                    collision: None,
                };
                *fixture.resources.skills.lock().expect("skills") = ResourceGroupRead {
                    items: Vec::new(),
                    diagnostics: vec![diagnostic(
                        DiagnosticKind::Collision,
                        "two skills named search",
                        "/a/SKILL.md",
                    )],
                };
                *fixture.resources.prompts.lock().expect("prompts") = ResourceGroupRead {
                    items: Vec::new(),
                    diagnostics: vec![diagnostic(
                        DiagnosticKind::Warning,
                        "prompt shadowed",
                        "/p.md",
                    )],
                };
                *fixture.resources.themes.lock().expect("themes") = ResourceGroupRead {
                    items: Vec::new(),
                    diagnostics: vec![diagnostic(DiagnosticKind::Error, "bad theme", "/t.json")],
                };
                *fixture.resources.extensions.lock().expect("extensions") = (
                    Vec::new(),
                    vec![("/broken.ts".to_string(), "cannot load".to_string())],
                );
                *fixture.session.command_diagnostics.lock().expect("diag") = vec![diagnostic(
                    DiagnosticKind::Warning,
                    "conflicts with built-in",
                    "/c.ts",
                )];
            },
            |shell, _, _| {
                shell.show_loaded_resources(false, true);
            },
        );
    }

    // -- exits ---------------------------------------------------------------------------------

    #[test]
    fn stop_teardown_order() {
        replay("exit.stop", |shell, _, _| {
            shell.stop("transcript");
        });
    }

    #[test]
    fn stop_teardown_not_initialized() {
        replay("exit.stop.notInitialized", |shell, _, _| {
            shell.lock().is_initialized = false;
            shell.stop("transcript");
        });
    }

    #[test]
    fn shutdown_interactive_exit() {
        replay("exit.shutdown.interactive", |shell, _, _| {
            futures::executor::block_on(shell.shutdown(false));
        });
    }

    #[test]
    fn shutdown_from_signal() {
        replay("exit.shutdown.fromSignal", |shell, _, log| {
            futures::executor::block_on(shell.shutdown(true));
            rec(log, json!(["caught", "process.exit(0)"]));
        });
    }

    #[test]
    fn shutdown_double() {
        replay("exit.shutdown.double", |shell, _, _| {
            futures::executor::block_on(shell.shutdown(false));
            futures::executor::block_on(shell.shutdown(false));
        });
    }

    #[test]
    fn exit_emergency() {
        replay("exit.emergency", |shell, _, _| {
            let shell = shell.clone();
            catching(move || shell.emergency_terminal_exit());
        });
    }

    #[test]
    fn exit_uncaught_first() {
        replay("exit.uncaught.first", |shell, _, _| {
            let shell = shell.clone();
            catching(move || shell.uncaught_crash("kaboom"));
        });
    }

    #[test]
    fn exit_uncaught_while_shutting_down() {
        replay("exit.uncaught.whileShuttingDown", |shell, _, _| {
            shell.lock().is_shutting_down = true;
            let shell = shell.clone();
            catching(move || shell.uncaught_crash("kaboom"));
        });
    }

    #[test]
    fn exit_check_shutdown_requested() {
        replay("exit.checkShutdownRequested", |shell, _, _| {
            shell.lock().shutdown_requested = true;
            futures::executor::block_on(shell.check_shutdown_requested());
        });
    }

    #[test]
    fn exit_fatal_runtime_error() {
        replay("exit.fatalRuntimeError", |shell, _, log| {
            let shell = shell.clone();
            // The platform exit seam returns normally in the fixture, so the
            // shell completes the teardown (the r20 harness's exit stub threw
            // after recording — the driver records the catch below).
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                futures::executor::block_on(
                    shell.handle_fatal_runtime_error("Failed to resume session", "gone"),
                )
            }));
            assert!(result.is_ok());
            rec(log, json!(["caught", "process.exit(1)"]));
        });
    }

    #[test]
    fn exit_resume_command() {
        replay("exit.resumeCommand", |_shell, fixture, log| {
            let command = super::super::interactive_mode::format_resume_command(
                fixture.manager.as_ref(),
                "pi",
                false,
                |_path| true,
            );
            rec(
                log,
                json!([
                    "resumeCommand",
                    command.map(Value::from).unwrap_or(Value::Null)
                ]),
            );
        });
    }
}
// ---------------------------------------------------------------------------
// r18 shell-oracle replay (upper half): drives the ported session shell
// through the same seams the r18 node harness recorded
// (`tests/fixtures/interactive_r18_oracle/shell_oracle.json`, 210 scenarios over
// verbatim upstream bodies spread on a fake `this`). Second fixture family:
// the r18 harness vocabulary differs from the r20 lower-half one in several
// places, reproduced here:
//
// - the drive's fake chalk renders `dim` as literal `[2m…[22m` (no escape
//   bytes) while theme colors keep real ANSI;
// - `session.prompt(text)` without options records ONE argument (the r20
//   driver recorded a trailing null);
// - `session.clearQueue` records the queue contents being cleared;
// - containers track real children; the `children`/`above`/`below` probes
//   render `describeArg` walks (Spacer → `{"height": 1}`, Text → its
//   instance fields, component stubs → their describe), while
//   `Container.addChild` renders the component `describe()` (`kind`-shaped);
// - plain-object children (drive-seeded `child`, the editor, the built-in
//   footer) record as `"[object Object]"`;
// - undefined constructor arguments record as `"undefined"`;
// - `this` lacks `maybeWarnAboutAnthropicSubscriptionAuth` (not part of the
//   r18 extraction), so the `cycleModel` success path throws and surfaces
//   `Error: this.maybeWarnAboutAnthropicSubscriptionAuth is not a function`
//   through showError — the session fixture reproduces that observable
//   artifact (harness-vocabulary emulation, disclosed).
// ---------------------------------------------------------------------------

mod upper_oracle {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use serde_json::{json, Value};

    use super::super::interactive_mode::{
        CompactionCostKind, CompactionCostNotice, CompactionQueuedMessage, ComponentKind,
        ComponentRef, ContainerId, EditorBorder, FocusTarget, InteractiveModeOptions, ModelRef,
        QueueSnapshot, ResourceDiagnostic, SessionStats, ShellClock, ShellEditor,
        ShellExtensionSurface, ShellPlatform, ShellSession, ShellShortcutSurface, ShellView,
    };
    use super::super::shell::{
        ChangelogSource, CommandSink, InteractiveMode, ShellCommand, ShellIo, WidgetContent,
        WidgetPlacement,
    };
    use super::super::theme::{load_builtin_theme, ColorMode};
    use crate::agent_core::types::{AgentMessage, ThinkingLevel};
    use crate::coding_agent::agent_session::{
        AgentSessionError, CompactionReason, CycleDirection, ModelCycleResult, ScopedModel,
    };
    use crate::coding_agent::extensions::types::StreamingDelivery;
    use crate::coding_agent::session_manager::SessionEntry;

    type Log = Arc<Mutex<Vec<Value>>>;

    fn rec(log: &Log, parts: Value) {
        log.lock().expect("log").push(parts);
    }

    /// The fixed driver date (drive_shell.ts `FIXED_MS`).
    const FIXED_MS: i64 = 1_790_604_147_438;

    fn upper_oracle() -> &'static Value {
        static ORACLE: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
        ORACLE.get_or_init(|| {
            serde_json::from_str(include_str!(
                "../../../../tests/fixtures/interactive_r18_oracle/shell_oracle.json"
            ))
            .expect("upper oracle parses")
        })
    }

    fn oracle_log(scenario: &str) -> Vec<Value> {
        upper_oracle()["scenarios"]
            .as_array()
            .expect("scenarios")
            .iter()
            .find(|s| s["name"] == json!(scenario))
            .unwrap_or_else(|| panic!("scenario {scenario} missing from oracle"))["log"]
            .as_array()
            .expect("log array")
            .clone()
    }

    /// environment-anchored: on POSIX the root-anchored fixture inputs stay
    /// `/...` and the shared scrub's root-anchored branch prepends the
    /// `<DRV>:/` placeholder to a leading `/`, rendering `<DRV>://...`; the
    /// win32 capture resolves onto the live drive and renders `<DRV>:/...`.
    /// Collapse the duplicated separator on BOTH sides (upstream-on-linux
    /// reports the same POSIX path) so the pin covers the path, not the scrub
    /// branch.
    fn collapse_drive_placeholder(value: &mut Value) {
        match value {
            Value::String(text) => {
                *text = text.replace("<DRV>://", "<DRV>:/");
            }
            Value::Array(items) => {
                for item in items {
                    collapse_drive_placeholder(item);
                }
            }
            Value::Object(entries) => {
                for (_, child) in entries.iter_mut() {
                    collapse_drive_placeholder(child);
                }
            }
            _ => {}
        }
    }

    /// The drive's `describeArg`: top-level `null` is the JS `undefined`
    /// sentinel and renders `"undefined"`; nested message payloads keep
    /// their JSON nulls; `__describe` markers render the handle shape.
    fn render_arg(value: &Value) -> Value {
        match value {
            Value::Array(items) => Value::Array(items.iter().map(render_arg).collect()),
            Value::Object(map) => {
                if let Some(kind) = map.get("__describe").and_then(Value::as_str) {
                    json!({ "kind": kind })
                } else {
                    Value::Object(
                        map.iter()
                            .map(|(k, v)| (k.clone(), describe_nested(v)))
                            .collect(),
                    )
                }
            }
            Value::Null => json!("undefined"),
            other => other.clone(),
        }
    }

    fn describe_nested(value: &Value) -> Value {
        match value {
            Value::Array(items) => Value::Array(items.iter().map(describe_nested).collect()),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), describe_nested(v)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    fn render_args(args: &Value) -> Vec<Value> {
        match args {
            Value::Array(items) => items.iter().map(render_arg).collect(),
            Value::Null => Vec::new(),
            other => vec![render_arg(other)],
        }
    }

    /// The default (`fakeEditor`) object describe from the harness.
    fn editor_describe(custom: bool) -> Value {
        let handler = if custom { "function" } else { "undefined" };
        json!({
            "_editorName": "<state>", "_state": "<state>",
            "onEscape": handler, "onCtrlD": handler, "onSubmit": "undefined",
            "onChange": "undefined", "onPasteImage": handler,
            "onExtensionShortcut": handler, "embedWorkingStatus": true,
            "actionHandlers": {},
            "getText": "function", "getExpandedText": "function", "setText": "function",
            "addToHistory": "function", "insertTextAtCursor": "function",
            "handleInput": "function", "setWorkingStatusIndicator": "function",
            "setAutocompleteProvider": "function", "setPaddingX": "function",
            "getPaddingX": "function", "setAutocompleteMaxVisible": "function",
            "getAutocompleteMaxVisible": "function", "onAction": "function",
        })
    }

    // ---------------------------------------------------------------------------
    // UpperView — real-container recording view (r18 vocabulary)
    // ---------------------------------------------------------------------------

    #[derive(Clone)]
    enum UpperChild {
        Spacer,
        Text {
            id: u64,
            text: String,
            pad_x: i64,
            pad_y: i64,
        },
        ExpandableText {
            text: String,
            pad_x: i64,
            pad_y: i64,
        },
        Border {
            color_tag: String,
        },
        Markdown {
            text: String,
            pad_x: i64,
        },
        Component(ComponentRef),
    }

    /// Plain-object children record as their JS `String()` form.
    fn plain_object_kind(kind: &str) -> bool {
        matches!(
            kind,
            "editor" | "customEditor" | "footer" | "reloadBox" | "child" | "widgetBox"
        )
    }

    struct UpperView {
        log: Log,
        ids: AtomicU64,
        loaded_resources: Mutex<Vec<UpperChild>>,
        chat: Mutex<Vec<UpperChild>>,
        pending_messages: Mutex<Vec<UpperChild>>,
        status: Mutex<Vec<UpperChild>>,
        widgets_above: Mutex<Vec<UpperChild>>,
        widgets_below: Mutex<Vec<UpperChild>>,
        header: Mutex<Vec<UpperChild>>,
        editor_container: Mutex<Vec<UpperChild>>,
        footer: Mutex<Vec<UpperChild>>,
        document: Mutex<Vec<UpperChild>>,
        /// The detached widget box (`containerName: "container"`).
        box_children: Mutex<Vec<UpperChild>>,
        chat_version: AtomicU64,
        /// Drive-flipped: whether the focused editor is the custom editor.
        editor_custom: AtomicBool,
        /// Drive-registered component describes (id → describe) for the
        /// focus / working-indicator probes (the stub's describe() output).
        describes: Mutex<BTreeMap<u64, Value>>,
        /// Drive-registered FIELD walks (id → fields) for stubs without a
        /// describe() method.
        field_describes: Mutex<BTreeMap<u64, Value>>,
        /// Drive-registered expandable stubs (the describe entry alone does
        /// not carry the instance's setExpanded method).
        expandable_ids: Mutex<std::collections::BTreeSet<u64>>,
        renderer_mode: Mutex<String>,
        overlay_count: Mutex<usize>,
        focused: Mutex<Option<ComponentRef>>,
        box_placed: std::sync::atomic::AtomicBool,
    }

    impl UpperView {
        fn new(log: Log) -> Self {
            Self {
                log,
                ids: AtomicU64::new(1),
                loaded_resources: Mutex::new(Vec::new()),
                chat: Mutex::new(Vec::new()),
                pending_messages: Mutex::new(Vec::new()),
                status: Mutex::new(Vec::new()),
                widgets_above: Mutex::new(Vec::new()),
                widgets_below: Mutex::new(Vec::new()),
                header: Mutex::new(Vec::new()),
                editor_container: Mutex::new(Vec::new()),
                footer: Mutex::new(Vec::new()),
                document: Mutex::new(Vec::new()),
                box_children: Mutex::new(Vec::new()),
                chat_version: AtomicU64::new(0),
                editor_custom: AtomicBool::new(false),
                describes: Mutex::new(BTreeMap::new()),
                field_describes: Mutex::new(BTreeMap::new()),
                expandable_ids: Mutex::new(std::collections::BTreeSet::new()),
                renderer_mode: Mutex::new("regular".to_string()),
                overlay_count: Mutex::new(0),
                focused: Mutex::new(None),
                box_placed: std::sync::atomic::AtomicBool::new(false),
            }
        }

        fn set_focused_component(&self, component: Option<ComponentRef>) {
            *self.focused.lock().expect("focused") = component;
        }

        fn id(&self) -> u64 {
            self.ids.fetch_add(1, Ordering::SeqCst)
        }

        fn container(&self, container: ContainerId) -> std::sync::MutexGuard<'_, Vec<UpperChild>> {
            match container {
                ContainerId::LoadedResources => &self.loaded_resources,
                ContainerId::Chat => &self.chat,
                ContainerId::PendingMessages => &self.pending_messages,
                ContainerId::Status => &self.status,
                ContainerId::WidgetsAbove => &self.widgets_above,
                ContainerId::WidgetsBelow => &self.widgets_below,
                ContainerId::Header => &self.header,
                ContainerId::EditorContainer => &self.editor_container,
                ContainerId::FooterContainer => &self.footer,
                ContainerId::Document => &self.document,
            }
            .lock()
            .expect("container")
        }

        fn bump(&self, container: ContainerId) {
            if container == ContainerId::Chat {
                self.chat_version.fetch_add(1, Ordering::SeqCst);
            }
        }

        fn describe_component(&self, component: &ComponentRef) -> Value {
            // The component describe() mode (addChild/removeChild/focus):
            // plain-object children (including the widget box) String()-ify;
            // component stubs return their `describe()` result verbatim
            // (drive_shell.ts describeArg).
            if plain_object_kind(&component.kind) {
                return json!("[object Object]");
            }
            if let Some(describe) = self.describes.lock().expect("describes").get(&component.id) {
                return describe.clone();
            }
            json!({ "kind": component.kind })
        }

        /// The `describeArg` rendering of a stored child (children probes).
        fn describe_child(&self, child: &UpperChild) -> Value {
            match child {
                UpperChild::Spacer => json!({ "height": 1 }),
                UpperChild::Text {
                    id: _,
                    text,
                    pad_x,
                    pad_y,
                } => json!({
                    "text": text,
                    "paddingX": pad_x,
                    "paddingY": pad_y,
                }),
                UpperChild::ExpandableText { text, pad_x, pad_y } => json!({
                    "text": text,
                    "paddingX": pad_x,
                    "paddingY": pad_y,
                    "getCollapsedText": "function",
                    "getExpandedText": "function",
                }),
                UpperChild::Border { color_tag } => json!({ "colorTag": color_tag }),
                UpperChild::Markdown { text, pad_x } => {
                    json!({ "text": text, "paddingX": pad_x })
                }
                // The describeArg walk of a stored child renders the
                // drive-registered fields (stubs without describe()) or the
                // describe(); the widget box walks as its container identity.
                UpperChild::Component(component) if component.kind == "widgetBox" => {
                    self.probe_box()
                }
                UpperChild::Component(component) => {
                    if let Some(fields) = self
                        .field_describes
                        .lock()
                        .expect("field describes")
                        .get(&component.id)
                    {
                        return fields.clone();
                    }
                    if let Some(describe) =
                        self.describes.lock().expect("describes").get(&component.id)
                    {
                        // `describeArg` returns the component `describe()`
                        // result verbatim.
                        return describe.clone();
                    }
                    json!({ "kind": component.kind })
                }
            }
        }

        fn describe_children(&self, children: &[UpperChild]) -> Vec<Value> {
            children
                .iter()
                .map(|child| self.describe_child(child))
                .collect()
        }

        /// `{container: name, children: [...]}` (the drive's `describeArg`
        /// of a `Container` instance).
        fn probe_container(&self, container: ContainerId) -> Value {
            let children = self.container(container).clone();
            json!({
                "container": container.as_str(),
                "children": self.describe_children(&children),
            })
        }

        fn probe_children(&self, container: ContainerId) -> Value {
            let children = self.container(container).clone();
            Value::Array(self.describe_children(&children))
        }

        /// The detached widget box probe.
        fn probe_box(&self) -> Value {
            let children = self.box_children.lock().expect("box").clone();
            json!({
                "container": "container",
                "children": self.describe_children(&children),
            })
        }

        fn register_describe(&self, id: u64, describe: Value) {
            self.describes
                .lock()
                .expect("describes")
                .insert(id, describe);
        }

        fn register_field_describe(&self, id: u64, fields: Value) {
            self.field_describes
                .lock()
                .expect("field describes")
                .insert(id, fields);
        }

        fn register_expandable(&self, id: u64) {
            self.expandable_ids.lock().expect("expandable").insert(id);
        }
    }

    impl ShellView for UpperView {
        fn request_render(&self, force: Option<bool>) {
            rec(
                &self.log,
                match force {
                    Some(force) => json!(["ui.requestRender", force]),
                    None => json!(["ui.requestRender"]),
                },
            );
        }
        fn invalidate(&self) {
            rec(&self.log, json!(["ui.invalidate"]));
        }
        fn render_now(&self) {
            rec(&self.log, json!(["ui.renderNow"]));
        }
        fn start(&self) {
            rec(&self.log, json!(["ui.start"]));
        }
        fn stop(&self, preserve_screen: bool) {
            rec(
                &self.log,
                json!(["ui.stop", { "preserveScreen": preserve_screen }]),
            );
        }
        fn set_clear_on_shrink(&self, enabled: bool) {
            rec(&self.log, json!(["ui.setClearOnShrink", enabled]));
        }
        fn set_show_hardware_cursor(&self, enabled: bool) {
            rec(&self.log, json!(["ui.setShowHardwareCursor", enabled]));
        }
        fn terminal_set_progress(&self, enabled: bool) {
            rec(&self.log, json!(["terminal.setProgress", enabled]));
        }
        fn terminal_set_title(&self, title: &str) {
            rec(&self.log, json!(["terminal.setTitle", title]));
        }
        fn drain_input(&self, ms: u64) {
            rec(&self.log, json!(["terminal.drainInput", ms]));
        }
        fn set_focus(&self, target: FocusTarget) {
            let described = match target {
                FocusTarget::Editor => editor_describe(self.editor_custom.load(Ordering::SeqCst)),
                FocusTarget::Component(component) => self.describe_component(&component),
                FocusTarget::None => Value::Null,
            };
            rec(&self.log, json!(["ui.setFocus", described]));
        }
        fn show_overlay(&self, component: &ComponentRef, _options: Option<Value>) {
            rec(
                &self.log,
                json!(["ui.showOverlay", { "kind": component.kind }, "undefined"]),
            );
        }
        fn hide_overlay(&self) {
            rec(&self.log, json!(["ui.hideOverlay"]));
        }
        fn add_input_listener(&self) -> u64 {
            rec(&self.log, json!(["ui.addInputListener", "function"]));
            self.id()
        }
        fn remove_input_listener(&self, _id: u64) {
            rec(&self.log, json!(["ui.removeInputListener"]));
        }
        fn container_version(&self, container: ContainerId) -> u64 {
            if container == ContainerId::Chat {
                self.chat_version.load(Ordering::SeqCst)
            } else {
                0
            }
        }
        fn container_clear(&self, container: ContainerId) {
            rec(&self.log, json!(["Container.clear", container.as_str()]));
            self.container(container).clear();
        }
        fn container_add_spacer(&self, container: ContainerId) -> u64 {
            self.bump(container);
            self.container(container).push(UpperChild::Spacer);
            rec(
                &self.log,
                json!([
                    "Container.addChild",
                    container.as_str(),
                    { "kind": "Spacer" }
                ]),
            );
            self.id()
        }
        fn container_add_text(
            &self,
            container: ContainerId,
            text: &str,
            pad_x: i64,
            pad_y: i64,
            truncated: bool,
        ) -> u64 {
            // The harness `describe()` matches `Text` before `TruncatedText`,
            // so truncated rows record as plain `Text`.
            let _ = truncated;
            self.bump(container);
            let id = self.id();
            self.container(container).push(UpperChild::Text {
                id,
                text: text.to_string(),
                pad_x,
                pad_y,
            });
            rec(
                &self.log,
                json!([
                    "Container.addChild",
                    container.as_str(),
                    {
                        "kind": "Text",
                        "text": text,
                        "paddingX": pad_x,
                        "paddingY": pad_y,
                    }
                ]),
            );
            id
        }
        fn container_add_expandable_text(
            &self,
            container: ContainerId,
            collapsed: &str,
            expanded: &str,
            initially_expanded: bool,
            pad_x: i64,
            pad_y: i64,
        ) -> u64 {
            // The upper drive's `describe()` of an ExpandableText adds the
            // body-getter function markers to the current text row.
            let text = if initially_expanded {
                expanded
            } else {
                collapsed
            };
            self.bump(container);
            let id = self.id();
            self.container(container).push(UpperChild::ExpandableText {
                text: text.to_string(),
                pad_x,
                pad_y,
            });
            rec(
                &self.log,
                json!([
                    "Container.addChild",
                    container.as_str(),
                    {
                        "kind": "Text",
                        "text": text,
                        "paddingX": pad_x,
                        "paddingY": pad_y,
                    }
                ]),
            );
            id
        }
        fn container_set_text(&self, _container: ContainerId, id: u64, text: &str) {
            let find_old = |children: &std::sync::Mutex<Vec<UpperChild>>| {
                let children = children.lock().expect("container");
                children.iter().find_map(|child| match child {
                    UpperChild::Text {
                        id: child_id,
                        text,
                        pad_x: _,
                        pad_y: _,
                    } if *child_id == id => Some(text.clone()),
                    _ => None,
                })
            };
            let old = find_old(&self.chat)
                .or_else(|| find_old(&self.pending_messages))
                .or_else(|| find_old(&self.status))
                .or_else(|| find_old(&self.widgets_above))
                .or_else(|| find_old(&self.widgets_below))
                .or_else(|| find_old(&self.header))
                .or_else(|| find_old(&self.editor_container))
                .or_else(|| find_old(&self.footer))
                .or_else(|| find_old(&self.loaded_resources));
            rec(
                &self.log,
                json!(["Text.setText", old.unwrap_or_default(), text]),
            );
            // Keep the mutated content for the next coalesce / probe.
            let mut all = [
                &self.chat,
                &self.pending_messages,
                &self.status,
                &self.widgets_above,
                &self.widgets_below,
                &self.header,
                &self.editor_container,
                &self.footer,
                &self.loaded_resources,
            ];
            for children in all.iter_mut() {
                let mut children = children.lock().expect("container");
                for child in children.iter_mut() {
                    if let UpperChild::Text {
                        id: child_id,
                        text: stored,
                        pad_x: _,
                        pad_y: _,
                    } = child
                    {
                        if *child_id == id {
                            *stored = text.to_string();
                        }
                    }
                }
            }
        }
        fn container_add_component(&self, container: ContainerId, component: &ComponentRef) {
            self.bump(container);
            if component.kind == "widgetBox" {
                self.box_placed.store(true, Ordering::SeqCst);
            }
            let described = self.describe_component(component);
            self.container(container)
                .push(UpperChild::Component(component.clone()));
            rec(
                &self.log,
                json!(["Container.addChild", container.as_str(), described]),
            );
        }
        fn container_remove_component(&self, container: ContainerId, component: &ComponentRef) {
            self.bump(container);
            let kind = component.kind.clone();
            let position = self
                .container(container)
                .iter()
                .position(|child| matches!(child, UpperChild::Component(c) if c.kind == kind));
            if let Some(position) = position {
                self.container(container).remove(position);
            }
            let described = self.describe_component(component);
            rec(
                &self.log,
                json!(["Container.removeChild", container.as_str(), described]),
            );
        }
        fn container_replace_child(
            &self,
            container: ContainerId,
            index: usize,
            component: &ComponentRef,
        ) {
            rec(
                &self.log,
                json!([
                    "Container.replaceChild",
                    container.as_str(),
                    index,
                    { "kind": component.kind }
                ]),
            );
        }
        fn container_children_len(&self, container: ContainerId) -> usize {
            self.container(container).len()
        }
        fn new_component(&self, kind: ComponentKind, args: Value) -> ComponentRef {
            let mut tuple = vec![Value::String(format!("new {}", kind.as_str()))];
            tuple.extend(render_args(&args));
            rec(&self.log, Value::Array(tuple));
            ComponentRef {
                kind: kind.as_str().to_string(),
                id: self.id(),
            }
        }
        fn update_component(&self, component: &ComponentRef, op: &str, args: Value) {
            // isExpandable modeling: a drive-registered stub is expandable
            // only when marked; upstream skips the setExpanded call otherwise.
            if op == "setExpanded"
                && self
                    .describes
                    .lock()
                    .expect("describes")
                    .contains_key(&component.id)
                && !self
                    .expandable_ids
                    .lock()
                    .expect("expandable")
                    .contains(&component.id)
            {
                return;
            }

            let mut tuple = vec![Value::String(format!("{}.{}", component.kind, op))];
            if op != "dispose" && !args.is_null() {
                tuple.extend(render_args(&args));
            }
            rec(&self.log, Value::Array(tuple));
        }
        fn emit(&self, tuple: Value) {
            // Track the detached widget box ("container") so the drive-side
            // probes render its live children.
            if tuple.get(0).and_then(Value::as_str) == Some("Container.addChild")
                && tuple.get(1).and_then(Value::as_str) == Some("container")
            {
                // A fresh upstream box identity resets the tracked rows.
                if self.box_placed.load(Ordering::SeqCst) {
                    self.box_placed.store(false, Ordering::SeqCst);
                    self.box_children.lock().expect("box").clear();
                }
                if let Some(child) = tuple.get(2) {
                    self.box_children
                        .lock()
                        .expect("box")
                        .push(box_child_from_record(child));
                }
            } else if tuple.get(0).and_then(Value::as_str) == Some("Container.clear")
                && tuple.get(1).and_then(Value::as_str) == Some("container")
            {
                self.box_children.lock().expect("box").clear();
            }
            rec(&self.log, tuple);
        }
        fn get_clear_on_shrink(&self) -> bool {
            false
        }
        fn idle_status_component(&self) -> ComponentRef {
            ComponentRef {
                kind: "IdleStatus".to_string(),
                id: 0,
            }
        }
        fn has_overlay_entries(&self) -> bool {
            *self.overlay_count.lock().expect("overlays") > 0
        }
        fn renderer_mode(&self) -> String {
            self.renderer_mode.lock().expect("mode").clone()
        }
        fn renderer_children(&self) -> Vec<ComponentRef> {
            Vec::new()
        }
        fn renderer_focused_component(&self) -> Option<ComponentRef> {
            self.focused.lock().expect("focused").clone()
        }
        fn renderer_terminal_id(&self) -> u64 {
            1
        }
        fn renderer_show_hardware_cursor(&self) -> bool {
            false
        }
        fn renderer_capture_render_state(&self) {}
        fn renderer_create(&self, mode: &str, _terminal: u64) -> u64 {
            *self.renderer_mode.lock().expect("mode") = mode.to_string();
            rec(
                &self.log,
                json!(["createInteractiveTui", {
                    "tuiMode": mode,
                    "showHardwareCursor": false,
                    "logDirectory": "/home/u/.pi/agent",
                    "terminal": {
                        "setProgress": "function",
                        "setTitle": "function",
                        "drainInput": "function",
                    },
                    "onRightClickPaste": "undefined",
                    "fullscreenCopyOnSelect": false,
                }]),
            );
            self.id()
        }
        fn renderer_become(&self, _id: u64) {}
        fn renderer_stop_preserving_screen(&self) {
            rec(&self.log, json!(["ui.stop", { "preserveScreen": true }]));
        }
        fn renderer_set_focus_none(&self) {
            rec(&self.log, json!(["ui.setFocus", "undefined"]));
        }
        fn renderer_clear(&self) {
            rec(&self.log, json!(["ui.clear"]));
        }
        fn renderer_set_layout_root_none(&self) {
            rec(&self.log, json!(["ui.setLayoutRoot", "undefined"]));
        }
        fn renderer_invalidate(&self) {
            rec(&self.log, json!(["ui.invalidate"]));
        }
        fn renderer_start(&self) {
            rec(&self.log, json!(["ui.start"]));
        }
        fn renderer_add_child(&self, container: ContainerId) {
            rec(
                &self.log,
                json!([
                    "renderer.addChild",
                    { "container": container.as_str(), "children": [] }
                ]),
            );
        }
        fn renderer_hide_overlay(&self) {
            let mut count = self.overlay_count.lock().expect("overlays");
            *count = count.saturating_sub(1);
            drop(count);
            rec(&self.log, json!(["renderer.hideOverlay"]));
        }
        fn renderer_render_now(&self) {
            rec(&self.log, json!(["renderer.renderNow"]));
        }
        fn renderer_add_child_by_name(&self, name: &str) {
            rec(
                &self.log,
                json!(["renderer.addChild", { "container": name, "children": [] }]),
            );
        }
        fn header_unshift(&self, component: &ComponentRef) {
            rec(
                &self.log,
                json!([
                    "Container.unshift",
                    "header",
                    { "kind": component.kind }
                ]),
            );
        }
        fn get_copy_on_select(&self) -> bool {
            false
        }
        fn has_active_selection(&self) -> bool {
            false
        }
        fn debug_render(&self) -> (usize, usize, Vec<String>) {
            (80, 24, vec!["line-one".to_string()])
        }
        fn container_components(&self, container: ContainerId) -> Vec<ComponentRef> {
            self.container(container)
                .iter()
                .filter_map(|child| match child {
                    UpperChild::Component(component) => Some(component.clone()),
                    _ => None,
                })
                .collect()
        }
        fn container_insert_at(
            &self,
            container: ContainerId,
            index: usize,
            component: &ComponentRef,
        ) {
            rec(
                &self.log,
                json!([
                    "Container.insertChild",
                    container.as_str(),
                    index,
                    { "kind": component.kind }
                ]),
            );
        }
        fn container_add_border(&self, container: ContainerId, _color_tag: Option<&str>) -> u64 {
            self.bump(container);
            self.container(container).push(UpperChild::Border {
                color_tag: "default".to_string(),
            });
            rec(
                &self.log,
                json!([
                    "Container.addChild",
                    container.as_str(),
                    { "kind": "DynamicBorder", "colorTag": "default" }
                ]),
            );
            self.id()
        }
        fn container_add_markdown(
            &self,
            container: ContainerId,
            text: &str,
            pad_x: i64,
            _pad_y: i64,
            _theme: &Value,
        ) -> u64 {
            self.bump(container);
            self.container(container).push(UpperChild::Markdown {
                text: text.to_string(),
                pad_x,
            });
            rec(
                &self.log,
                json!([
                    "Container.addChild",
                    container.as_str(),
                    { "kind": "Markdown", "text": text, "paddingX": pad_x }
                ]),
            );
            self.id()
        }
        fn container_remove_at(&self, _container: ContainerId, _component: &ComponentRef) {}
        fn container_replace_child_unrecorded(
            &self,
            container: ContainerId,
            index: usize,
            component: &ComponentRef,
        ) {
            let mut children = self.container(container);
            if index < children.len() {
                children[index] = UpperChild::Component(component.clone());
            }
        }
        fn session_identity(&self) -> u64 {
            1
        }
        fn user_input_resolved(&self, _slot: u64, _text: &str) {}
        fn component_handle_input(&self, component: &ComponentRef, data: &str) -> bool {
            if component.kind == "target" {
                rec(&self.log, json!(["target.handleInput", data]));
                return true;
            }
            false
        }
    }

    /// Rebuilds a detached-box child from the recorded addChild describe.
    fn box_child_from_record(child: &Value) -> UpperChild {
        let kind = child.get("kind").and_then(Value::as_str).unwrap_or("");
        match kind {
            "Spacer" => UpperChild::Spacer,
            "Text" => UpperChild::Text {
                id: 0,
                text: child
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                pad_x: child
                    .get("paddingX")
                    .and_then(Value::as_i64)
                    .unwrap_or_default(),
                pad_y: child
                    .get("paddingY")
                    .and_then(Value::as_i64)
                    .unwrap_or_default(),
            },
            _ => UpperChild::Component(ComponentRef {
                kind: kind.to_string(),
                id: 0,
            }),
        }
    }

    /// The upper-family extension runner surface (transformer knob +
    /// command diagnostics for the loaded-resources scenario).
    struct UpperExtensions {
        transformers: Vec<String>,
        command_diagnostics: Vec<ResourceDiagnostic>,
    }

    impl ShellExtensionSurface for UpperExtensions {
        fn has_command(&self, name: &str) -> bool {
            name == "extcmd" || name == "extcmd2"
        }
        fn registered_commands(&self) -> Vec<super::super::interactive_mode::ExtensionCommandInfo> {
            Vec::new()
        }
        fn command_diagnostics(&self) -> Vec<ResourceDiagnostic> {
            self.command_diagnostics.clone()
        }
        fn shortcut_diagnostics(&self) -> Vec<ResourceDiagnostic> {
            Vec::new()
        }
        fn markdown_transformers(&self) -> Vec<String> {
            self.transformers.clone()
        }
        fn has_entry_renderer(&self, _custom_type: &str) -> bool {
            false
        }
        fn has_message_renderer(&self, _custom_type: &str) -> bool {
            false
        }
        fn emit(&self, _tuple: Value) {}
    }

    // ---------------------------------------------------------------------------
    // UpperEditor — the recording editor with drive knobs
    // ---------------------------------------------------------------------------

    struct UpperEditor {
        log: Log,
        name: &'static str,
        view: Arc<UpperView>,
        text: Mutex<String>,
        embeds: AtomicBool,
    }

    impl UpperEditor {
        fn new(log: Log, name: &'static str, view: Arc<UpperView>) -> Self {
            Self {
                log,
                name,
                view,
                text: Mutex::new(String::new()),
                embeds: AtomicBool::new(true),
            }
        }

        /// Drive-side text seed (upstream `(t.defaultEditor)._state.text = …`
        /// writes no log).
        fn seed_text(&self, text: &str) {
            *self.text.lock().expect("text") = text.to_string();
        }
    }

    impl ShellEditor for UpperEditor {
        fn get_text(&self) -> String {
            self.text.lock().expect("text").clone()
        }
        fn get_expanded_text(&self) -> String {
            self.get_text()
        }
        fn set_text(&self, text: &str) {
            rec(&self.log, json!([format!("{}.setText", self.name), text]));
            *self.text.lock().expect("text") = text.to_string();
        }
        fn add_to_history(&self, text: &str) {
            rec(
                &self.log,
                json!([format!("{}.addToHistory", self.name), text]),
            );
        }
        fn insert_text_at_cursor(&self, text: &str) {
            rec(
                &self.log,
                json!([format!("{}.insertTextAtCursor", self.name), text]),
            );
        }
        fn set_border_color(&self, border: EditorBorder) {
            rec(
                &self.log,
                json!([format!("{}.borderColor", self.name), border.tag()]),
            );
        }
        fn border_color(&self) -> Option<String> {
            None
        }
        fn handle_input(&self, data: &str) {
            rec(
                &self.log,
                json!([format!("{}.handleInput", self.name), data]),
            );
        }
        fn set_working_status_indicator(&self, indicator: Option<ComponentRef>) {
            let described = match indicator {
                Some(component) => self
                    .view
                    .field_describes
                    .lock()
                    .expect("field describes")
                    .get(&component.id)
                    .cloned()
                    .unwrap_or_else(|| json!({ "kind": component.kind })),
                None => json!("undefined"),
            };
            rec(
                &self.log,
                json!([
                    format!("{}.setWorkingStatusIndicator", self.name),
                    described
                ]),
            );
        }
        fn set_autocomplete_provider(&self) {
            rec(
                &self.log,
                json!([format!("{}.setAutocompleteProvider", self.name), "provider"]),
            );
        }
        fn set_padding_x(&self, px: i64) {
            rec(&self.log, json!([format!("{}.setPaddingX", self.name), px]));
        }
        fn set_autocomplete_max_visible(&self, n: i64) {
            rec(
                &self.log,
                json!([format!("{}.setAutocompleteMaxVisible", self.name), n]),
            );
        }
        fn get_padding_x(&self) -> i64 {
            2
        }
        fn get_autocomplete_max_visible(&self) -> i64 {
            6
        }
        fn on_action(&self, action: &'static str) {
            rec(
                &self.log,
                json!([format!("{}.onAction", self.name), action, "handler"]),
            );
        }
        fn set_on_escape(&self) {}
        fn set_on_ctrl_d(&self) {}
        fn set_on_submit(&self) {}
        fn set_on_change(&self) {}
        fn set_on_paste_image(&self) {}
        fn set_on_extension_shortcut(&self, _enabled: bool) {}
        fn embeds_working_status(&self) -> bool {
            self.embeds.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    // ---------------------------------------------------------------------------
    // UpperCommands — the `cmd.<name>` recording command sink
    // ---------------------------------------------------------------------------

    struct UpperCommands {
        log: Log,
    }

    impl CommandSink for UpperCommands {
        fn run(&self, command: ShellCommand) {
            let undefined = json!("undefined");
            let tuple = match &command {
                ShellCommand::Settings => json!(["cmd.showSettingsSelector"]),
                ShellCommand::ScopedModels => json!(["cmd.showModelsSelector"]),
                ShellCommand::Model(arg) => json!([
                    "cmd.handleModelCommand",
                    arg.clone().map(Value::String).unwrap_or(undefined)
                ]),
                ShellCommand::Thinking(arg) => json!([
                    "cmd.handleThinkingCommand",
                    arg.clone().map(Value::String).unwrap_or(json!("undefined"))
                ]),
                ShellCommand::Export(text) => json!(["cmd.handleExportCommand", text]),
                ShellCommand::Import(text) => json!(["cmd.handleImportCommand", text]),
                ShellCommand::Share => json!(["cmd.handleShareCommand"]),
                ShellCommand::Bug(arg) => json!([
                    "cmd.handleBugCommand",
                    arg.clone().map(Value::String).unwrap_or(undefined.clone())
                ]),
                ShellCommand::Copy { .. } => json!(["cmd.handleCopyCommand"]),
                ShellCommand::Name(text) => json!(["cmd.handleNameCommand", text]),
                ShellCommand::Session => json!(["cmd.handleSessionCommand"]),
                ShellCommand::Changelog => json!(["cmd.handleChangelogCommand"]),
                ShellCommand::Hotkeys => json!(["cmd.handleHotkeysCommand"]),
                ShellCommand::UserMessageSelector => json!(["cmd.showUserMessageSelector"]),
                ShellCommand::Clone => json!(["cmd.handleCloneCommand"]),
                ShellCommand::Tree => json!(["cmd.showTreeSelector"]),
                ShellCommand::Trust => json!(["cmd.showTrustSelector"]),
                ShellCommand::Login(arg) => json!([
                    "cmd.handleLoginCommand",
                    arg.clone().map(Value::String).unwrap_or(json!("undefined"))
                ]),
                ShellCommand::OAuthLogout => json!(["cmd.showOAuthSelector", "logout"]),
                ShellCommand::Clear => json!(["cmd.handleClearCommand"]),
                ShellCommand::Compact(arg) => json!([
                    "cmd.handleCompactCommand",
                    arg.clone().map(Value::String).unwrap_or(json!("undefined"))
                ]),
                ShellCommand::Reload => json!(["cmd.handleReloadCommand"]),
                ShellCommand::Debug => json!(["cmd.handleDebugCommand"]),
                ShellCommand::ArminSaysHi => json!(["cmd.handleArminSaysHi"]),
                ShellCommand::DementedDelves => json!(["cmd.handleDementedDelves"]),
                ShellCommand::SessionSelector => json!(["cmd.showSessionSelector"]),
                ShellCommand::Bash {
                    command,
                    exclude_from_context,
                } => json!(["cmd.handleBashCommand", command, exclude_from_context]),
                ShellCommand::TreeSelector => json!(["cmd.showTreeSelector"]),
                ShellCommand::ModelSelector => json!(["cmd.showModelSelector"]),
                ShellCommand::Init => json!(["cmd.init"]),
            };
            rec(&self.log, tuple);
        }
    }

    // ---------------------------------------------------------------------------
    // UpperSession — the r18 fake session (queues, cycle knobs, recording)
    // ---------------------------------------------------------------------------

    /// The `cycleModel` outcome knob.
    // The `Success` variant carries `ModelCycleResult` by value on purpose: it
    // mirrors the upstream fake session's `cycleModel` return shape, and boxing
    // it would distort that mirror for a test-only knob. Variant-size skew is
    // expected, hence the lint allowance (fixture fidelity, not an oversight).
    #[allow(clippy::large_enum_variant)]
    enum CycleOutcome {
        None,
        Success(ModelCycleResult),
        Error(String),
    }

    struct UpperSession {
        log: Log,
        streaming: AtomicBool,
        compacting: AtomicBool,
        bash_running: AtomicBool,
        retry_attempt: AtomicU32,
        steering: Mutex<Vec<String>>,
        follow_up: Mutex<Vec<String>>,
        cycle_thinking: Mutex<Option<ThinkingLevel>>,
        cycle_model_outcome: Mutex<CycleOutcome>,
        scoped: Mutex<Vec<ScopedModel>>,
        messages: Mutex<Vec<AgentMessage>>,
        /// `prompt(text)` calls matching this text reject without recording
        /// (the throwing driver override).
        prompt_error: Mutex<Option<(String, String)>>,
        /// The extension markdown transformers (drive_shell's fake runner
        /// exposes `extTransformer`; the scenario-local runner exposes none).
        ext_transformers: Mutex<Vec<String>>,
        /// `extensionRunner.getCommandDiagnostics` for the loaded-resources
        /// scenario.
        command_diagnostics: Mutex<Vec<ResourceDiagnostic>>,
        runtime: Arc<super::RecModelRuntime>,
        resources: Arc<super::RecResources>,
        shortcuts: Arc<super::RecShortcuts>,
    }

    impl UpperSession {
        fn new(
            log: Log,
            runtime: Arc<super::RecModelRuntime>,
            resources: Arc<super::RecResources>,
            shortcuts: Arc<super::RecShortcuts>,
        ) -> Self {
            Self {
                log,
                streaming: AtomicBool::new(false),
                compacting: AtomicBool::new(false),
                bash_running: AtomicBool::new(false),
                retry_attempt: AtomicU32::new(0),
                steering: Mutex::new(Vec::new()),
                follow_up: Mutex::new(Vec::new()),
                cycle_thinking: Mutex::new(None),
                cycle_model_outcome: Mutex::new(CycleOutcome::None),
                scoped: Mutex::new(Vec::new()),
                messages: Mutex::new(Vec::new()),
                prompt_error: Mutex::new(None),
                ext_transformers: Mutex::new(vec!["extTransformer".to_string()]),
                command_diagnostics: Mutex::new(Vec::new()),
                runtime,
                resources,
                shortcuts,
            }
        }

        /// The r18 fake `this` lacks `maybeWarnAboutAnthropicSubscriptionAuth`
        /// (outside the r18 extraction), so upstream's
        /// `void this.maybeWarn…(model)` throws a TypeError inside the
        /// `cycleModel` try block, which surfaces through `showError`. The
        /// fixture reproduces that observable artifact.
        fn record_missing_warn_artifact(&self) {
            let theme =
                load_builtin_theme("dark", Some(ColorMode::Truecolor)).expect("built-in theme");
            let text = theme
                .fg(
                    "error",
                    "Error: this.maybeWarnAboutAnthropicSubscriptionAuth is not a function",
                )
                .expect("error color");
            rec(
                &self.log,
                json!(["Container.addChild", "chat", { "kind": "Spacer" }]),
            );
            rec(
                &self.log,
                json!([
                    "Container.addChild",
                    "chat",
                    {
                        "kind": "Text",
                        "text": text,
                        "paddingX": 1,
                        "paddingY": 0,
                    }
                ]),
            );
            rec(&self.log, json!(["ui.requestRender"]));
        }
    }

    impl ShellSession for UpperSession {
        fn is_streaming(&self) -> bool {
            self.streaming.load(std::sync::atomic::Ordering::SeqCst)
        }
        fn is_compacting(&self) -> bool {
            self.compacting.load(std::sync::atomic::Ordering::SeqCst)
        }
        fn is_bash_running(&self) -> bool {
            self.bash_running.load(std::sync::atomic::Ordering::SeqCst)
        }
        fn is_idle(&self) -> bool {
            !self.is_streaming()
        }
        fn thinking_level(&self) -> ThinkingLevel {
            ThinkingLevel::Medium
        }
        fn retry_attempt(&self) -> u32 {
            self.retry_attempt.load(std::sync::atomic::Ordering::SeqCst)
        }
        fn pending_message_count(&self) -> usize {
            0
        }
        fn scoped_models(&self) -> Vec<ScopedModel> {
            self.scoped.lock().expect("scoped").clone()
        }
        fn steering_messages(&self) -> Vec<String> {
            self.steering.lock().expect("steering").clone()
        }
        fn follow_up_messages(&self) -> Vec<String> {
            self.follow_up.lock().expect("follow up").clone()
        }
        fn clear_queue(&self) -> (Vec<String>, Vec<String>) {
            let steering = self.steering.lock().expect("steering").clone();
            let follow_up = self.follow_up.lock().expect("follow up").clone();
            rec(
                &self.log,
                json!([
                    "session.clearQueue",
                    { "steering": steering, "followUp": follow_up }
                ]),
            );
            self.steering.lock().expect("steering").clear();
            self.follow_up.lock().expect("follow up").clear();
            (steering, follow_up)
        }
        fn prompt(
            &self,
            text: String,
            streaming_behavior: Option<StreamingDelivery>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<(), AgentSessionError>> + Send + '_>,
        > {
            let log = self.log.clone();
            let throwing = self
                .prompt_error
                .lock()
                .expect("prompt error")
                .clone()
                .filter(|(pattern, _)| *pattern == text)
                .map(|(_, error)| error);
            Box::pin(async move {
                if let Some(error) = throwing {
                    // The throwing driver override replaces the logging stub.
                    return Err(AgentSessionError::Upstream(error));
                }
                rec(
                    &log,
                    match streaming_behavior {
                        None => json!(["session.prompt", text]),
                        Some(behavior) => json!([
                            "session.prompt",
                            text,
                            { "streamingBehavior": match behavior {
                                StreamingDelivery::Steer => "steer",
                                StreamingDelivery::FollowUp => "followUp",
                            } }
                        ]),
                    },
                );
                Ok(())
            })
        }
        fn steer(
            &self,
            text: String,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<(), AgentSessionError>> + Send + '_>,
        > {
            let log = self.log.clone();
            Box::pin(async move {
                rec(&log, json!(["session.steer", text]));
                Ok(())
            })
        }
        fn follow_up(
            &self,
            text: String,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<(), AgentSessionError>> + Send + '_>,
        > {
            let log = self.log.clone();
            Box::pin(async move {
                rec(&log, json!(["session.followUp", text]));
                Ok(())
            })
        }
        fn abort(&self) {
            // Upstream routes the restore-abort through this.agent.abort();
            // the r18 harness records that seam as "agent.abort".
            rec(&self.log, json!(["agent.abort"]));
        }
        fn abort_bash(&self) {
            rec(&self.log, json!(["session.abortBash"]));
        }
        fn abort_compaction(&self) {
            rec(&self.log, json!(["session.abortCompaction"]));
        }
        fn abort_retry(&self) {
            rec(&self.log, json!(["session.abortRetry"]));
        }
        fn cycle_thinking_level(&self) -> Option<ThinkingLevel> {
            *self.cycle_thinking.lock().expect("cycle thinking")
        }
        fn cycle_model(
            &self,
            _direction: CycleDirection,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<Option<ModelCycleResult>, AgentSessionError>,
                    > + Send
                    + '_,
            >,
        > {
            let outcome = match &*self.cycle_model_outcome.lock().expect("cycle outcome") {
                CycleOutcome::None => Ok(None),
                CycleOutcome::Success(result) => Ok(Some(result.clone())),
                CycleOutcome::Error(message) => Err(AgentSessionError::Upstream(message.clone())),
            };
            Box::pin(async move { outcome })
        }
        fn extensions(&self) -> Arc<dyn ShellExtensionSurface> {
            Arc::new(UpperExtensions {
                transformers: self.ext_transformers.lock().expect("transformers").clone(),
                command_diagnostics: self.command_diagnostics.lock().expect("diag").clone(),
            })
        }
        fn maybe_warn_anthropic_subscription_auth(&self, _provider: Option<&str>) {
            self.record_missing_warn_artifact();
        }
        fn subscribe(&self) -> u64 {
            rec(&self.log, json!(["session.subscribe"]));
            1
        }
        fn unsubscribe(&self, _slot: u64) {
            rec(&self.log, json!(["session.unsubscribe"]));
        }
        fn model(&self) -> Option<ModelRef> {
            None
        }
        fn model_runtime(&self) -> Arc<dyn super::super::interactive_mode::ShellModelRuntime> {
            self.runtime.clone()
        }
        fn resources(&self) -> Arc<dyn super::super::interactive_mode::ShellResources> {
            self.resources.clone()
        }
        fn shortcuts(&self) -> Arc<dyn ShellShortcutSurface> {
            self.shortcuts.clone()
        }
        fn available_thinking_levels(&self) -> Vec<ThinkingLevel> {
            vec![
                ThinkingLevel::Off,
                ThinkingLevel::Minimal,
                ThinkingLevel::Low,
                ThinkingLevel::Medium,
                ThinkingLevel::High,
            ]
        }
        fn set_model(
            &self,
            _model: &ModelRef,
            _persist: bool,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + '_>>
        {
            Box::pin(async { Ok(()) })
        }
        fn set_thinking_level(&self, _level: ThinkingLevel, _persist: bool) -> Result<(), String> {
            Ok(())
        }
        fn set_scoped_models(&self, _models: &[ModelRef]) {}
        fn auto_compaction_enabled(&self) -> bool {
            true
        }
        fn set_auto_compaction_enabled(&self, _enabled: bool) {}
        fn steering_mode(&self) -> Value {
            Value::Null
        }
        fn follow_up_mode(&self) -> Value {
            Value::Null
        }
        fn set_steering_mode(&self, _mode: Value) {}
        fn set_follow_up_mode(&self, _mode: Value) {}
        fn user_messages_for_forking(
            &self,
        ) -> Vec<super::super::interactive_mode::ForkableUserMessage> {
            Vec::new()
        }
        fn session_stats(&self) -> SessionStats {
            SessionStats::default()
        }
        fn last_assistant_text(&self) -> Option<String> {
            None
        }
        fn set_session_name(&self, _name: &str) {}
        fn emit(&self, tuple: Value) {
            rec(&self.log, tuple);
        }
        fn navigate_tree(
            &self,
            _entry_id: &str,
            _summarize: bool,
            _custom_instructions: Option<&str>,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<super::super::interactive_mode::NavigateOutcome, String>,
                    > + Send
                    + '_,
            >,
        > {
            Box::pin(async { Ok(super::super::interactive_mode::NavigateOutcome::default()) })
        }
        fn abort_branch_summary(&self) {}
        fn compact(
            &self,
            _custom_instructions: Option<&str>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + '_>>
        {
            Box::pin(async { Ok(()) })
        }
        fn execute_bash(
            &self,
            _command: &str,
            _exclude_from_context: bool,
            _chunk_sink: &dyn Fn(&str),
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<super::super::interactive_mode::BashOutcome, String>,
                    > + Send
                    + '_,
            >,
        > {
            Box::pin(async {
                Ok(super::super::interactive_mode::BashOutcome {
                    exit_code: Some(0),
                    cancelled: false,
                    output: String::new(),
                    truncated: false,
                    full_output_path: None,
                })
            })
        }
        fn record_bash_result(
            &self,
            _command: &str,
            _result: &super::super::interactive_mode::BashOutcome,
            _exclude_from_context: bool,
        ) {
        }
        fn reload(
            &self,
            _before_session_start: Option<&dyn Fn()>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + '_>>
        {
            Box::pin(async { Ok(()) })
        }
        fn export_to_jsonl(&self, path: &str) -> Result<String, String> {
            Ok(path.to_string())
        }
        fn build_bug_report_bundle(
            &self,
            options: crate::coding_agent::modes::interactive::bug_report::BugReportOptions,
            summary: Option<String>,
        ) -> futures::future::BoxFuture<
            '_,
            Result<
                crate::coding_agent::modes::interactive::interactive_mode::BugReportOutcome,
                String,
            >,
        > {
            let _ = (options, summary);
            Box::pin(async {
                Ok(
                    crate::coding_agent::modes::interactive::interactive_mode::BugReportOutcome {
                        report_id: "bug-1".to_string(),
                        created_at: "2026-01-01T00:00:00.000Z".to_string(),
                        zip_path: Some("pi-bug-report-bug-1.zip".to_string()),
                        crash_count: 0,
                    },
                )
            })
        }
        fn export_to_html(
            &self,
            _path: Option<&str>,
            _theme_name: &str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>
        {
            Box::pin(async { Ok("/out.html".to_string()) })
        }
        fn tool_definition(&self, name: &str) -> Value {
            json!({ "name": name, "builtIn": true })
        }
        fn context_usage(&self) -> Option<Value> {
            None
        }
        fn system_prompt(&self) -> String {
            "sys".to_string()
        }
        fn messages(&self) -> Vec<AgentMessage> {
            self.messages.lock().expect("messages").clone()
        }
        fn wait_for_idle(
            &self,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
            Box::pin(async {})
        }
        fn bind_extensions(
            &self,
            context: Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
            let log = self.log.clone();
            Box::pin(async move {
                rec(
                    &log,
                    json!(["session.bindExtensions", render_arg(&context)]),
                );
            })
        }
        fn detect_cache_miss(
            &self,
            _message: &AgentMessage,
        ) -> Option<super::super::interactive_mode::CacheMiss> {
            None
        }
        fn collect_cache_misses(&self) -> Vec<(Value, super::super::interactive_mode::CacheMiss)> {
            Vec::new()
        }
    }

    // ---------------------------------------------------------------------------
    // UpperPlatform / UpperClock — the patched-process seam
    // ---------------------------------------------------------------------------

    struct UpperPlatform {
        log: Log,
        is_windows: AtomicBool,
        stdout_is_tty: AtomicBool,
        pi_offline: AtomicBool,
        signal_handlers_registered: AtomicBool,
    }

    impl UpperPlatform {
        fn new(log: Log) -> Self {
            Self {
                log,
                is_windows: AtomicBool::new(false),
                stdout_is_tty: AtomicBool::new(false),
                pi_offline: AtomicBool::new(false),
                signal_handlers_registered: AtomicBool::new(false),
            }
        }
    }

    impl UpperPlatform {
        fn signal_handlers_registered(&self) -> bool {
            self.signal_handlers_registered
                .load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl ShellPlatform for UpperPlatform {
        fn exit(&self, code: i32) -> bool {
            // The r18 harness's fake `process.exit` records and returns, so
            // upstream `shutdown(fromSignal)` falls through to the
            // interactive quit path (the oracle's double teardown tail).
            rec(&self.log, json!(["process.exit", code]));
            true
        }
        fn is_windows(&self) -> bool {
            self.is_windows.load(std::sync::atomic::Ordering::SeqCst)
        }
        fn now_ms(&self) -> i64 {
            FIXED_MS
        }
        fn stdout_is_tty(&self) -> bool {
            self.stdout_is_tty.load(std::sync::atomic::Ordering::SeqCst)
        }
        fn kill_tracked_detached_children(&self) {}
        fn copy_to_clipboard(
            &self,
            _text: &str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + '_>>
        {
            Box::pin(async { Ok(()) })
        }
        fn read_clipboard_text(
            &self,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send + '_>>
        {
            Box::pin(async { Some("clip text".to_string()) })
        }
        fn read_clipboard_image(
            &self,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Option<(String, Vec<u8>)>> + Send + '_>,
        > {
            Box::pin(async { None })
        }
        fn pi_offline(&self) -> bool {
            self.pi_offline.load(std::sync::atomic::Ordering::SeqCst)
        }
        fn has_trust_requiring_project_resources(&self, _cwd: &str) -> bool {
            false
        }
        fn register_signal_handlers(&self) -> Vec<u64> {
            self.signal_handlers_registered
                .store(true, std::sync::atomic::Ordering::SeqCst);
            // The r18 harness's fake process: SIGTERM/SIGHUP prepend, two
            // stdout listeners, one stderr listener, uncaughtException.
            rec(
                &self.log,
                json!(["process.prependListener", "SIGTERM", "function"]),
            );
            rec(
                &self.log,
                json!(["process.prependListener", "SIGHUP", "function"]),
            );
            rec(&self.log, json!(["process.stdout.on"]));
            rec(&self.log, json!(["process.stderr.on"]));
            rec(
                &self.log,
                json!(["process.prependListener", "uncaughtException", "function"]),
            );
            vec![1, 2, 3, 4, 5, 6]
        }
        fn unregister_signal_handlers(&self, _ids: &[u64]) {
            if !self.signal_handlers_registered() {
                return;
            }
            self.signal_handlers_registered
                .store(false, std::sync::atomic::Ordering::SeqCst);
            rec(&self.log, json!(["process.off", "SIGTERM", "function"]));
            rec(&self.log, json!(["process.off", "SIGHUP", "function"]));
            rec(&self.log, json!(["process.stdout.off"]));
            rec(&self.log, json!(["process.stderr.off"]));
            rec(
                &self.log,
                json!(["process.off", "uncaughtException", "function"]),
            );
        }
        fn suspend(&self) -> Option<()> {
            Some(())
        }
        fn now_iso(&self) -> String {
            "2026-09-28T14:02:27.438Z".to_string()
        }
        fn basename(&self, path: &str) -> String {
            path.rsplit(['/', '\\'])
                .next()
                .unwrap_or_default()
                .to_string()
        }
        fn join_path(&self, parts: &[&str]) -> String {
            // node win32 `path.join` (the harness ran on win32): normalize
            // separators, keep the leading segment.
            let normalized: Vec<String> = parts.iter().map(|p| p.replace('/', "\\")).collect();
            normalized.join("\\")
        }
        fn file_exists(&self, path: &str) -> bool {
            // The harness fs stub: only the session file exists.
            path.ends_with("abc123.jsonl")
        }
        fn stop_theme_watcher(&self) -> Result<(), String> {
            // The r18 extraction does not carry stopThemeWatcher; the call
            // throws and unwinds handleFatalRuntimeError after showError.
            Err("stopThemeWatcher is not defined".to_string())
        }
    }

    struct UpperClock(std::sync::Mutex<i64>);

    impl UpperClock {
        fn new() -> Self {
            Self(std::sync::Mutex::new(FIXED_MS))
        }
        fn set(&self, ms: i64) {
            *self.0.lock().expect("clock") = ms;
        }
    }

    impl ShellClock for UpperClock {
        fn now_ms(&self) -> i64 {
            *self.0.lock().expect("clock")
        }
    }

    /// The r18 changelog document (drive_shell.ts deps).
    struct UpperChangelog;

    impl ChangelogSource for UpperChangelog {
        fn entries(&self) -> Vec<(String, String)> {
            vec![
                ("1.2.0".to_string(), "old".to_string()),
                ("1.1.0".to_string(), "older".to_string()),
            ]
        }
        fn new_entries(&self, last_version: &str) -> Vec<(String, String)> {
            self.entries()
                .into_iter()
                .filter(|(version, _)| version.as_str() > last_version)
                .collect()
        }
        fn normalize_links(&self, content: &str) -> String {
            content.to_string()
        }
    }

    fn key_display(_action: &str) -> String {
        // The harness keybindings resolve every action to `["ctrl+c"]`.
        "Ctrl+C".to_string()
    }

    // ---------------------------------------------------------------------------
    // Fixture assembly + replay driver
    // ---------------------------------------------------------------------------

    struct UpperFixture {
        log: Log,
        view: Arc<UpperView>,
        default_editor: Arc<UpperEditor>,
        settings: Arc<super::RecSettings>,
        manager: Arc<super::RecSessionManager>,
        resources: Arc<super::RecResources>,
        session: Arc<UpperSession>,
        platform: Arc<UpperPlatform>,
        clock: Arc<UpperClock>,
    }

    impl UpperFixture {
        fn new(log: Log) -> Self {
            let view = Arc::new(UpperView::new(log.clone()));
            let default_editor =
                Arc::new(UpperEditor::new(log.clone(), "defaultEditor", view.clone()));
            let settings = Arc::new(super::RecSettings::new(log.clone()));
            let manager = Arc::new(super::RecSessionManager::new());
            let runtime = Arc::new(super::RecModelRuntime::new(log.clone()));
            runtime.snapshot.lock().expect("snapshot").clear();
            let resources = Arc::new(super::RecResources::default());
            let shortcuts = Arc::new(super::RecShortcuts::new(log.clone()));
            let session = Arc::new(UpperSession::new(
                log.clone(),
                runtime.clone(),
                resources.clone(),
                shortcuts.clone(),
            ));
            let platform = Arc::new(UpperPlatform::new(log.clone()));
            Self {
                log,
                view,
                default_editor,
                settings,
                manager,
                resources,
                session,
                platform,
                clock: Arc::new(UpperClock::new()),
            }
        }

        fn build(&self, options: InteractiveModeOptions) -> Arc<InteractiveMode> {
            let io = ShellIo {
                session: self.session.clone(),
                session_manager: self.manager.clone(),
                settings: self.settings.clone(),
                view: self.view.clone(),
                host: Arc::new(super::RecHost::new(self.log.clone())),
                commands: Arc::new(UpperCommands {
                    log: self.log.clone(),
                }),
                clock: self.clock.clone(),
                platform: self.platform.clone(),
                default_editor: self.default_editor.clone(),
                default_model_per_provider: vec![
                    ("anthropic".to_string(), "claude-opus-4-8".to_string()),
                    ("radius".to_string(), "balanced".to_string()),
                ],
                auth_path: "/home/u/.pi/agent/auth.json".to_string(),
                docs_path: "/docs".to_string(),
                debug_log_path: "/tmp/pi-debug.log".to_string(),
                app_name: "pi".to_string(),
                app_title: "Pi".to_string(),
                version: "1.2.3".to_string(),
                home: "/home/u".to_string(),
                changelog: Box::new(UpperChangelog),
                package_updates: Arc::new(super::FixturePackageUpdates::new(self.log.clone())),
                key_display: Box::new(key_display),
                theme: std::sync::RwLock::new(
                    load_builtin_theme("dark", Some(ColorMode::Truecolor)).expect("built-in theme"),
                ),
                cache_stats: None,
                // The drive's fake chalk has no color support (literal
                // brackets).
                chalk_styler: Box::new(|text: &str| format!("[2m{text}[22m")),
            };
            let shell = Arc::new(InteractiveMode::new(io, options));
            shell.force_initialized();
            shell
        }
    }

    /// Replays one upper scenario and asserts byte-parity with the oracle.
    fn replay_upper_with(
        name: &str,
        options: InteractiveModeOptions,
        customize: impl FnOnce(&UpperFixture),
        drive: impl FnOnce(&Arc<InteractiveMode>, &UpperFixture) + Send,
    ) {
        let log: Log = Arc::new(Mutex::new(Vec::new()));
        let fixture = UpperFixture::new(log.clone());
        customize(&fixture);
        let shell = fixture.build(options);
        drive(&shell, &fixture);
        let mut ours: Vec<Value> = log.lock().expect("log").clone();
        let mut expected = oracle_log(name);
        // environment-anchored: both sides normalized. Root-relative fixture
        // inputs (`/outside/AGENTS.md`) resolve onto the live drive while the
        // capture stores the capture machine's `C:` form, so both sides go
        // through the shared anchor scrub before the byte comparison.
        for entry in ours.iter_mut() {
            crate::coding_agent::oracle_scrub::scrub_value(entry);
        }
        for entry in expected.iter_mut() {
            crate::coding_agent::oracle_scrub::scrub_value(entry);
        }
        // environment-anchored: both sides normalized. On POSIX the
        // root-anchored inputs stay `/...` and the shared scrub's
        // root-anchored branch prepends the `<DRV>:/` placeholder to a
        // leading `/`, rendering `<DRV>://...`; the win32 capture resolves
        // onto the live drive and renders `<DRV>:/...`. Collapse the
        // duplicated separator on BOTH sides (upstream-on-linux reports the
        // same POSIX path) so the pin covers the path, not the scrub branch.
        for entry in ours.iter_mut() {
            collapse_drive_placeholder(entry);
        }
        for entry in expected.iter_mut() {
            collapse_drive_placeholder(entry);
        }
        let failures: Vec<String> = ours
            .iter()
            .enumerate()
            .zip(expected.iter())
            .filter(|(ours_pair, expected)| ours_pair.1 != *expected)
            .map(|((index, actual), expected)| {
                format!(
                    "  [{index}] ours:   {}\n      oracle: {}",
                    serde_json::to_string(actual).unwrap_or_default(),
                    serde_json::to_string(expected).unwrap_or_default()
                )
            })
            .collect();
        assert!(
            failures.is_empty() && ours.len() == expected.len(),
            "scenario {name} diverged ({} vs {} entries):\n{}",
            ours.len(),
            expected.len(),
            failures.join("\n")
        );
    }

    /// Replays one upper scenario against the default fixture.
    fn replay_upper(name: &str, drive: impl FnOnce(&Arc<InteractiveMode>, &UpperFixture) + Send) {
        replay_upper_with(name, InteractiveModeOptions::default(), |_| {}, drive);
    }

    fn queue_snapshot_value(snapshot: &QueueSnapshot) -> Value {
        json!({
            "steering": snapshot.steering,
            "followUp": snapshot.follow_up,
        })
    }

    fn compaction_queue_value(messages: &[CompactionQueuedMessage]) -> Value {
        Value::Array(
            messages
                .iter()
                .map(|message| {
                    json!({
                        "text": message.text,
                        "mode": message.mode.as_str(),
                    })
                })
                .collect(),
        )
    }

    /// The scenario message literal as the lossless custom capture (the
    /// typed standard-role shapes require provider fields the scenario
    /// literals omit; the custom capture serializes the literal verbatim).
    fn message(value: Value) -> AgentMessage {
        let role = value
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("custom")
            .to_string();
        custom_message(&role, value)
    }

    fn custom_message(role: &str, value: Value) -> AgentMessage {
        let mut data = serde_json::Map::new();
        if let Value::Object(entries) = value {
            for (key, entry) in entries {
                if key != "role" {
                    data.insert(key, entry);
                }
            }
        }
        AgentMessage::Custom(crate::agent_core::types::CustomAgentMessage {
            role: role.to_string(),
            data,
        })
    }

    /// The typed user message (text extraction reads the typed content).
    fn user_message(content: Value) -> AgentMessage {
        AgentMessage::User(crate::ai::types::message::UserMessage {
            content: serde_json::from_value(content).expect("user content parses"),
            timestamp: 0,
        })
    }

    /// The harness `WorkingStatusIndicator`-style model for drive-seeded
    /// indicator describes.
    fn indicator_describe(kind: &str, methods: &[&str]) -> Value {
        let mut map = serde_json::Map::new();
        map.insert("kind".to_string(), json!(kind));
        for method in methods {
            map.insert(method.to_string(), json!("function"));
        }
        Value::Object(map)
    }

    // -- submit ladder -------------------------------------------------------

    #[test]
    fn submit_empty() {
        replay_upper("submit.empty", |shell, _| {
            futures::executor::block_on(shell.submit("   "));
        });
    }

    #[test]
    fn submit_settings() {
        replay_upper("submit.settings", |shell, _| {
            futures::executor::block_on(shell.submit("/settings"));
        });
    }

    #[test]
    fn submit_model_arg() {
        replay_upper("submit.model.arg", |shell, _| {
            futures::executor::block_on(shell.submit("/model gpt-5"));
        });
    }

    #[test]
    fn submit_model_bare() {
        replay_upper("submit.model.bare", |shell, _| {
            futures::executor::block_on(shell.submit("/model"));
        });
    }

    #[test]
    fn submit_thinking_arg() {
        replay_upper("submit.thinking.arg", |shell, _| {
            futures::executor::block_on(shell.submit("/thinking high"));
        });
    }

    #[test]
    fn submit_bash_normal() {
        replay_upper("submit.bash.normal", |shell, _| {
            futures::executor::block_on(shell.submit("!ls -la"));
        });
    }

    #[test]
    fn submit_bash_excluded() {
        replay_upper("submit.bash.excluded", |shell, _| {
            futures::executor::block_on(shell.submit("!!rm -rf /tmp/x"));
        });
    }

    #[test]
    fn submit_bash_bang_only_falls_through() {
        replay_upper("submit.bash.bang-only-falls-through", |shell, _| {
            futures::executor::block_on(shell.submit("!"));
        });
    }

    #[test]
    fn submit_bash_conflict() {
        replay_upper("submit.bash.conflict", |shell, fixture| {
            fixture
                .session
                .bash_running
                .store(true, std::sync::atomic::Ordering::SeqCst);
            futures::executor::block_on(shell.submit("!echo hi"));
        });
    }

    #[test]
    fn submit_normal_idle() {
        replay_upper("submit.normal.idle", |shell, _| {
            futures::executor::block_on(shell.submit("hello world"));
        });
    }

    #[test]
    fn submit_normal_idle_no_callback_queues() {
        replay_upper("submit.normal.idle.no-callback-queues", |shell, _| {
            futures::executor::block_on(shell.submit("queued for later"));
        });
    }

    #[test]
    fn submit_streaming_steer() {
        replay_upper("submit.streaming.steer", |shell, fixture| {
            fixture
                .session
                .streaming
                .store(true, std::sync::atomic::Ordering::SeqCst);
            futures::executor::block_on(shell.submit("mid-stream input"));
        });
    }

    #[test]
    fn submit_compacting_queues() {
        replay_upper("submit.compacting.queues", |shell, fixture| {
            fixture
                .session
                .compacting
                .store(true, std::sync::atomic::Ordering::SeqCst);
            futures::executor::block_on(shell.submit("hold this"));
        });
    }

    #[test]
    fn submit_compacting_extension_command() {
        replay_upper("submit.compacting.extension-command", |shell, fixture| {
            fixture
                .session
                .compacting
                .store(true, std::sync::atomic::Ordering::SeqCst);
            futures::executor::block_on(shell.submit("/extcmd run"));
        });
    }

    #[test]
    fn submit_command_tail_not_recognized() {
        replay_upper("submit.command.tail-not-recognized", |shell, _| {
            futures::executor::block_on(shell.submit("/settingsfoo"));
        });
    }

    #[test]
    fn submit_share() {
        replay_upper("submit.share", |shell, _| {
            futures::executor::block_on(shell.submit("/share"));
        });
    }

    #[test]
    fn submit_name_arg() {
        replay_upper("submit.name.arg", |shell, _| {
            futures::executor::block_on(shell.submit("/name my session"));
        });
    }

    #[test]
    fn submit_bash_clears_bash_mode() {
        replay_upper("submit.bash.clears-bash-mode", |shell, fixture| {
            // The scenario drives `defaultEditor.onChange("!ls")`, whose body
            // toggles bash mode before the submit.
            shell.lock().is_bash_mode = true;
            futures::executor::block_on(shell.submit("!ls"));
            rec(
                &fixture.log,
                json!(["final.isBashMode", shell.state_snapshot().is_bash_mode]),
            );
        });
    }

    // -- escape ring -----------------------------------------------------------

    #[test]
    fn escape_streaming() {
        replay_upper("escape.streaming", |shell, fixture| {
            shell.setup_key_handlers();
            fixture
                .session
                .streaming
                .store(true, std::sync::atomic::Ordering::SeqCst);
            futures::executor::block_on(shell.on_escape_pressed());
        });
    }

    #[test]
    fn escape_bash_running() {
        replay_upper("escape.bash-running", |shell, fixture| {
            shell.setup_key_handlers();
            fixture
                .session
                .bash_running
                .store(true, std::sync::atomic::Ordering::SeqCst);
            futures::executor::block_on(shell.on_escape_pressed());
        });
    }

    #[test]
    fn escape_bash_mode() {
        replay_upper("escape.bash-mode", |shell, fixture| {
            shell.setup_key_handlers();
            shell.lock().is_bash_mode = true;
            futures::executor::block_on(shell.on_escape_pressed());
            rec(
                &fixture.log,
                json!(["final.isBashMode", shell.state_snapshot().is_bash_mode]),
            );
        });
    }

    #[test]
    fn escape_empty_once() {
        replay_upper("escape.empty.once", |shell, _| {
            shell.setup_key_handlers();
            futures::executor::block_on(shell.on_escape_pressed());
        });
    }

    #[test]
    fn escape_empty_double_fork() {
        replay_upper("escape.empty.double-fork", |shell, _| {
            shell.setup_key_handlers();
            futures::executor::block_on(shell.on_escape_pressed());
            futures::executor::block_on(shell.on_escape_pressed());
        });
    }

    #[test]
    fn escape_empty_double_tree() {
        replay_upper_with(
            "escape.empty.double-tree",
            InteractiveModeOptions::default(),
            |fixture| {
                *fixture.settings.double_escape_action.lock().expect("knob") =
                    Some("tree".to_string());
            },
            |shell, _| {
                shell.setup_key_handlers();
                futures::executor::block_on(shell.on_escape_pressed());
                futures::executor::block_on(shell.on_escape_pressed());
            },
        );
    }

    #[test]
    fn escape_empty_double_none() {
        replay_upper_with(
            "escape.empty.double-none",
            InteractiveModeOptions::default(),
            |fixture| {
                *fixture.settings.double_escape_action.lock().expect("knob") =
                    Some("none".to_string());
            },
            |shell, _| {
                shell.setup_key_handlers();
                futures::executor::block_on(shell.on_escape_pressed());
                futures::executor::block_on(shell.on_escape_pressed());
            },
        );
    }

    #[test]
    fn escape_nonempty_idle() {
        replay_upper("escape.nonempty.idle", |shell, fixture| {
            shell.setup_key_handlers();
            fixture.default_editor.seed_text("draft");
            futures::executor::block_on(shell.on_escape_pressed());
        });
    }

    #[test]
    fn keyring_action_table() {
        replay_upper("keyring.action-table", |shell, fixture| {
            shell.setup_key_handlers();
            let mut actions: Vec<&str> =
                super::super::interactive_mode::INPUT_RING_ACTIONS.to_vec();
            actions.sort_unstable();
            rec(&fixture.log, json!(["actionHandlerNames", actions]));
        });
    }

    // -- ctrl-c / ctrl-d / startup / ctrl-z / shutdown / signals ---------------

    #[test]
    fn ctrlc_first_clears() {
        replay_upper("ctrlc.first-clears", |shell, fixture| {
            fixture.default_editor.seed_text("draft");
            futures::executor::block_on(shell.handle_ctrl_c());
            rec(
                &fixture.log,
                json!([
                    "final.lastSigintTimeSet",
                    shell.state_snapshot().last_sigint_time > 0
                ]),
            );
        });
    }

    #[test]
    fn ctrlc_double_shuts_down() {
        replay_upper("ctrlc.double-shuts-down", |shell, fixture| {
            fixture
                .platform
                .stdout_is_tty
                .store(true, std::sync::atomic::Ordering::SeqCst);
            fixture.clock.set(1000);
            futures::executor::block_on(shell.handle_ctrl_c());
            fixture.clock.set(1200);
            futures::executor::block_on(shell.handle_ctrl_c());
        });
    }

    #[test]
    fn ctrlc_second_late_clears() {
        replay_upper("ctrlc.second-late-clears", |shell, fixture| {
            fixture.clock.set(1000);
            futures::executor::block_on(shell.handle_ctrl_c());
            fixture.clock.set(2000);
            futures::executor::block_on(shell.handle_ctrl_c());
        });
    }

    #[test]
    fn ctrld_shuts_down() {
        replay_upper("ctrld.shuts-down", |shell, fixture| {
            fixture
                .platform
                .stdout_is_tty
                .store(true, std::sync::atomic::Ordering::SeqCst);
            futures::executor::block_on(shell.handle_ctrl_d());
        });
    }

    #[test]
    fn startup_submit() {
        replay_upper("startup-submit", |shell, _| {
            shell.handle_startup_submit("early input");
        });
    }

    #[test]
    fn ctrlz_win32() {
        replay_upper("ctrlz.win32", |shell, fixture| {
            fixture
                .platform
                .is_windows
                .store(true, std::sync::atomic::Ordering::SeqCst);
            shell.handle_ctrl_z();
        });
    }

    #[test]
    fn shutdown_graceful() {
        replay_upper("shutdown.graceful", |shell, fixture| {
            fixture
                .platform
                .stdout_is_tty
                .store(true, std::sync::atomic::Ordering::SeqCst);
            futures::executor::block_on(shell.shutdown(false));
        });
    }

    #[test]
    fn shutdown_from_signal() {
        replay_upper("shutdown.from-signal", |shell, fixture| {
            fixture
                .platform
                .stdout_is_tty
                .store(true, std::sync::atomic::Ordering::SeqCst);
            futures::executor::block_on(shell.shutdown(true));
        });
    }

    #[test]
    fn shutdown_resumed_session_no_command() {
        replay_upper("shutdown.resumed-session-no-command", |shell, fixture| {
            fixture
                .manager
                .persisted
                .store(false, std::sync::atomic::Ordering::SeqCst);
            fixture
                .platform
                .stdout_is_tty
                .store(true, std::sync::atomic::Ordering::SeqCst);
            futures::executor::block_on(shell.shutdown(false));
        });
    }

    #[test]
    fn signals_register_and_unregister() {
        replay_upper("signals.register-and-unregister", |shell, _| {
            shell.register_signal_handlers();
            shell.unregister_signal_handlers();
        });
    }

    #[test]
    fn signals_dead_terminal_error_codes() {
        replay_upper("signals.dead-terminal-error-codes", |_shell, fixture| {
            let code = super::super::interactive_mode::is_dead_terminal_error_code;
            rec(
                &fixture.log,
                json!([
                    "deadTerminal",
                    code(Some("EPIPE")),
                    code(Some("EIO")),
                    code(Some("ENOTCONN")),
                    code(Some("ENOENT")),
                    code(None),
                    code(Some("x")),
                    code(Some("")),
                ]),
            );
        });
    }

    #[test]
    fn check_shutdown_requested_idle() {
        replay_upper("check-shutdown-requested.idle", |shell, fixture| {
            futures::executor::block_on(shell.check_shutdown_requested());
            shell.lock().shutdown_requested = true;
            fixture
                .platform
                .stdout_is_tty
                .store(true, std::sync::atomic::Ordering::SeqCst);
            futures::executor::block_on(shell.check_shutdown_requested());
        });
    }

    // -- queues ------------------------------------------------------------------

    #[test]
    fn queue_get_combines() {
        replay_upper("queue.get-combines", |shell, fixture| {
            *fixture.session.steering.lock().expect("bag") =
                vec!["s1".to_string(), "s2".to_string()];
            *fixture.session.follow_up.lock().expect("bag") = vec!["f1".to_string()];
            shell.lock().compaction_queued_messages = vec![
                CompactionQueuedMessage {
                    text: "cs1".to_string(),
                    mode: super::super::interactive_mode::QueueMode::Steer,
                },
                CompactionQueuedMessage {
                    text: "cf1".to_string(),
                    mode: super::super::interactive_mode::QueueMode::FollowUp,
                },
            ];
            let result = shell.get_all_queued_messages();
            rec(
                &fixture.log,
                json!(["result", queue_snapshot_value(&result)]),
            );
        });
    }

    #[test]
    fn queue_clear_all() {
        replay_upper("queue.clear-all", |shell, fixture| {
            *fixture.session.steering.lock().expect("bag") = vec!["s1".to_string()];
            *fixture.session.follow_up.lock().expect("bag") =
                vec!["f1".to_string(), "f2".to_string()];
            shell.lock().compaction_queued_messages = vec![CompactionQueuedMessage {
                text: "cs1".to_string(),
                mode: super::super::interactive_mode::QueueMode::Steer,
            }];
            let result = shell.clear_all_queues();
            rec(
                &fixture.log,
                json!(["result", queue_snapshot_value(&result)]),
            );
            rec(
                &fixture.log,
                json!(["remaining", shell.lock().compaction_queued_messages.len()]),
            );
        });
    }

    #[test]
    fn queue_restore_to_editor() {
        replay_upper("queue.restore-to-editor", |shell, fixture| {
            *fixture.session.steering.lock().expect("bag") = vec!["s1".to_string()];
            *fixture.session.follow_up.lock().expect("bag") = vec!["f1".to_string()];
            shell.lock().compaction_queued_messages = vec![CompactionQueuedMessage {
                text: "cs1".to_string(),
                mode: super::super::interactive_mode::QueueMode::Steer,
            }];
            fixture.default_editor.seed_text("current draft");
            let count = shell.restore_queued_messages_to_editor(false, None);
            rec(&fixture.log, json!(["count", count]));
            rec(
                &fixture.log,
                json!(["editorText", fixture.default_editor.get_text()]),
            );
        });
    }

    #[test]
    fn queue_restore_empty_abort() {
        replay_upper("queue.restore-empty-abort", |shell, fixture| {
            let count = shell.restore_queued_messages_to_editor(true, None);
            rec(&fixture.log, json!(["count", count]));
        });
    }

    #[test]
    fn queue_restore_abort_with_queue() {
        replay_upper("queue.restore-abort-with-queue", |shell, fixture| {
            *fixture.session.steering.lock().expect("bag") = vec!["s1".to_string()];
            let count = shell.restore_queued_messages_to_editor(true, None);
            rec(&fixture.log, json!(["count", count]));
        });
    }

    #[test]
    fn queue_restore_skips_empty_current() {
        replay_upper("queue.restore-skips-empty-current", |shell, fixture| {
            *fixture.session.steering.lock().expect("bag") = vec!["s1".to_string()];
            fixture.default_editor.seed_text("   ");
            let count = shell.restore_queued_messages_to_editor(false, None);
            rec(&fixture.log, json!(["count", count]));
            rec(
                &fixture.log,
                json!(["editorText", fixture.default_editor.get_text()]),
            );
        });
    }

    #[test]
    fn queue_dequeue_empty() {
        replay_upper("queue.dequeue-empty", |shell, _| {
            shell.handle_dequeue();
        });
    }

    #[test]
    fn queue_dequeue_restores() {
        replay_upper("queue.dequeue-restores", |shell, fixture| {
            *fixture.session.steering.lock().expect("bag") =
                vec!["s1".to_string(), "s2".to_string()];
            *fixture.session.follow_up.lock().expect("bag") = vec!["f1".to_string()];
            shell.handle_dequeue();
            rec(
                &fixture.log,
                json!(["editorText", fixture.default_editor.get_text()]),
            );
        });
    }

    #[test]
    fn queue_compaction_message() {
        replay_upper("queue.compaction-message", |shell, fixture| {
            shell.queue_compaction_message(
                "queued text",
                super::super::interactive_mode::QueueMode::Steer,
            );
            let queued = shell.lock().compaction_queued_messages.clone();
            rec(
                &fixture.log,
                json!(["queued", compaction_queue_value(&queued)]),
            );
            rec(
                &fixture.log,
                json!(["editorText", fixture.default_editor.get_text()]),
            );
        });
    }

    #[test]
    fn queue_follow_up_streaming() {
        replay_upper("queue.follow-up.streaming", |shell, fixture| {
            fixture
                .session
                .streaming
                .store(true, std::sync::atomic::Ordering::SeqCst);
            fixture.default_editor.seed_text("follow up please");
            futures::executor::block_on(shell.handle_follow_up());
        });
    }

    #[test]
    fn queue_follow_up_streaming_extension_command() {
        replay_upper(
            "queue.follow-up.streaming-extension-command",
            |shell, fixture| {
                fixture
                    .session
                    .streaming
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                fixture.default_editor.seed_text("/extcmd do");
                futures::executor::block_on(shell.handle_follow_up());
            },
        );
    }

    #[test]
    fn queue_follow_up_compacting() {
        replay_upper("queue.follow-up.compacting", |shell, fixture| {
            fixture
                .session
                .compacting
                .store(true, std::sync::atomic::Ordering::SeqCst);
            fixture.default_editor.seed_text("wait for me");
            futures::executor::block_on(shell.handle_follow_up());
        });
    }

    #[test]
    fn queue_follow_up_compacting_extension_command() {
        replay_upper(
            "queue.follow-up.compacting-extension-command",
            |shell, fixture| {
                fixture
                    .session
                    .compacting
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                fixture.default_editor.seed_text("/extcmd do");
                futures::executor::block_on(shell.handle_follow_up());
            },
        );
    }

    #[test]
    fn queue_follow_up_idle_acts_as_submit() {
        replay_upper("queue.follow-up.idle-acts-as-submit", |shell, fixture| {
            fixture.default_editor.seed_text("idle follow");
            futures::executor::block_on(shell.handle_follow_up());
        });
    }

    #[test]
    fn queue_follow_up_empty() {
        replay_upper("queue.follow-up.empty", |shell, _| {
            futures::executor::block_on(shell.handle_follow_up());
        });
    }

    #[test]
    fn queue_is_extension_command() {
        replay_upper("queue.is-extension-command", |shell, fixture| {
            let results = [
                shell.is_extension_command("/extcmd a"),
                shell.is_extension_command("/unknown"),
                shell.is_extension_command("plain"),
                shell.is_extension_command("/"),
            ];
            rec(
                &fixture.log,
                json!(["results", results[0], results[1], results[2], results[3]]),
            );
        });
    }

    #[test]
    fn queue_flush_will_retry() {
        replay_upper("queue.flush.will-retry", |shell, _| {
            shell.lock().compaction_queued_messages = vec![
                CompactionQueuedMessage {
                    text: "/extcmd prep".to_string(),
                    mode: super::super::interactive_mode::QueueMode::Steer,
                },
                CompactionQueuedMessage {
                    text: "real prompt".to_string(),
                    mode: super::super::interactive_mode::QueueMode::FollowUp,
                },
                CompactionQueuedMessage {
                    text: "steered".to_string(),
                    mode: super::super::interactive_mode::QueueMode::Steer,
                },
            ];
            futures::executor::block_on(shell.flush_compaction_queue(true));
        });
    }

    #[test]
    fn queue_flush_normal() {
        replay_upper("queue.flush.normal", |shell, _| {
            shell.lock().compaction_queued_messages = vec![
                CompactionQueuedMessage {
                    text: "/extcmd prep".to_string(),
                    mode: super::super::interactive_mode::QueueMode::Steer,
                },
                CompactionQueuedMessage {
                    text: "real prompt".to_string(),
                    mode: super::super::interactive_mode::QueueMode::FollowUp,
                },
                CompactionQueuedMessage {
                    text: "extra".to_string(),
                    mode: super::super::interactive_mode::QueueMode::Steer,
                },
                CompactionQueuedMessage {
                    text: "extra2".to_string(),
                    mode: super::super::interactive_mode::QueueMode::FollowUp,
                },
            ];
            futures::executor::block_on(shell.flush_compaction_queue(false));
        });
    }

    #[test]
    fn queue_flush_all_extension_commands() {
        replay_upper("queue.flush.all-extension-commands", |shell, _| {
            shell.lock().compaction_queued_messages = vec![
                CompactionQueuedMessage {
                    text: "/extcmd a".to_string(),
                    mode: super::super::interactive_mode::QueueMode::Steer,
                },
                CompactionQueuedMessage {
                    text: "/extcmd b".to_string(),
                    mode: super::super::interactive_mode::QueueMode::FollowUp,
                },
            ];
            futures::executor::block_on(shell.flush_compaction_queue(false));
        });
    }

    #[test]
    fn queue_flush_empty() {
        replay_upper("queue.flush.empty", |shell, _| {
            futures::executor::block_on(shell.flush_compaction_queue(false));
        });
    }

    #[test]
    fn queue_flush_prompt_error_restores() {
        replay_upper("queue.flush.prompt-error-restores", |shell, fixture| {
            *fixture.session.prompt_error.lock().expect("knob") =
                Some(("boom prompt".to_string(), "prompt failed".to_string()));
            shell.lock().compaction_queued_messages = vec![
                CompactionQueuedMessage {
                    text: "boom prompt".to_string(),
                    mode: super::super::interactive_mode::QueueMode::Steer,
                },
                CompactionQueuedMessage {
                    text: "after".to_string(),
                    mode: super::super::interactive_mode::QueueMode::FollowUp,
                },
            ];
            futures::executor::block_on(shell.flush_compaction_queue(false));
            let restored = shell.lock().compaction_queued_messages.clone();
            rec(
                &fixture.log,
                json!(["restored", compaction_queue_value(&restored)]),
            );
        });
    }

    #[test]
    fn queue_flush_pending_bash() {
        replay_upper("queue.flush-pending-bash", |shell, fixture| {
            let one = ComponentRef {
                kind: "BashStub".to_string(),
                id: 1,
            };
            let two = ComponentRef {
                kind: "BashStub".to_string(),
                id: 2,
            };
            fixture
                .view
                .register_describe(1, json!({ "kind": "BashStub", "id": 1 }));
            fixture
                .view
                .register_describe(2, json!({ "kind": "BashStub", "id": 2 }));
            fixture
                .view
                .container_add_component(ContainerId::Chat, &one);
            fixture
                .view
                .container_add_component(ContainerId::PendingMessages, &two);
            shell.lock().pending_bash_components = vec![two];
            shell.flush_pending_bash_components();
            rec(
                &fixture.log,
                json!([
                    "pendingChildren",
                    fixture
                        .view
                        .container_children_len(ContainerId::PendingMessages),
                    "chatChildren",
                    fixture.view.probe_children(ContainerId::Chat),
                ]),
            );
        });
    }

    // -- status coalescing + notifications --------------------------------------

    #[test]
    fn status_coalesce_consecutive() {
        replay_upper("status.coalesce-consecutive", |shell, fixture| {
            shell.show_status("first");
            shell.show_status("second");
            shell.show_status("third");
            rec(
                &fixture.log,
                json!(["children", fixture.view.probe_children(ContainerId::Chat)]),
            );
        });
    }

    #[test]
    fn status_not_coalesced_after_other() {
        replay_upper("status.not-coalesced-after-other", |shell, fixture| {
            shell.show_status("first");
            fixture
                .view
                .container_add_text(ContainerId::Chat, "user message", 1, 0, false);
            shell.show_status("second");
            rec(
                &fixture.log,
                json!(["children", fixture.view.probe_children(ContainerId::Chat)]),
            );
        });
    }

    #[test]
    fn status_managed_tool() {
        replay_upper("status.managed-tool", |shell, fixture| {
            shell.show_managed_tool_status(false, "downloading fd");
            shell.show_managed_tool_status(true, "checksum mismatch");
            rec(
                &fixture.log,
                json!(["children", fixture.view.probe_children(ContainerId::Chat)]),
            );
        });
    }

    #[test]
    fn status_error_and_warning() {
        replay_upper("status.error-and-warning", |shell, fixture| {
            shell.show_error("boom");
            shell.show_warning("careful");
            rec(
                &fixture.log,
                json!(["children", fixture.view.probe_children(ContainerId::Chat)]),
            );
        });
    }

    #[test]
    fn status_clear_editor() {
        replay_upper("status.clear-editor", |shell, fixture| {
            fixture.default_editor.seed_text("text");
            shell.clear_editor();
        });
    }

    #[test]
    fn status_new_version_notification() {
        replay_upper("status.new-version-notification", |shell, fixture| {
            shell.show_new_version_notification("9.9.9", Some("  Fixed stuff.  "));
            rec(
                &fixture.log,
                json!(["children", fixture.view.probe_children(ContainerId::Chat)]),
            );
        });
    }

    #[test]
    fn status_package_update_notification() {
        replay_upper("status.package-update-notification", |shell, fixture| {
            shell.show_package_update_notification(&["ext-a".to_string(), "ext-b".to_string()]);
            rec(
                &fixture.log,
                json!(["children", fixture.view.probe_children(ContainerId::Chat)]),
            );
        });
    }

    use crate::coding_agent::agent_session::{AgentSessionEvent, SummarizationRetrySource};

    fn event_value(json_value: Value) -> AgentMessage {
        message(json_value)
    }

    fn session_event(event: AgentSessionEvent) -> AgentSessionEvent {
        event
    }

    fn compaction_entry(
        id: &str,
        parent_id: Option<&str>,
        timestamp: &str,
        summary: &str,
        tokens_before: i64,
        usage: Option<Value>,
    ) -> SessionEntry {
        SessionEntry::Compaction(crate::coding_agent::session_manager::CompactionEntry {
            id: id.to_string(),
            parent_id: parent_id.map(str::to_string),
            timestamp: timestamp.to_string(),
            summary: summary.to_string(),
            first_kept_entry_id: Some("kept".to_string()),
            tokens_before,
            details: None,
            usage: usage.map(|value| serde_json::from_value(value).expect("usage parses")),
            from_hook: None,
            system_message: None,
            first_kept_entry_index: None,
        })
    }

    fn message_entry(role_json: Value) -> SessionEntry {
        SessionEntry::Message(crate::coding_agent::session_manager::MessageEntry {
            id: "m1".to_string(),
            parent_id: None,
            timestamp: "2025-01-01T00:00:00Z".to_string(),
            message: user_message(role_json.get("content").cloned().unwrap_or(Value::Null)),
        })
    }

    // -- events -------------------------------------------------------------------

    #[test]
    fn event_turn_start() {
        replay_upper("event.turn-start", |shell, _| {
            futures::executor::block_on(
                shell.handle_event(&session_event(AgentSessionEvent::TurnStart)),
            );
        });
    }

    #[test]
    fn event_turn_start_progress_setting() {
        replay_upper_with(
            "event.turn-start.progress-setting",
            InteractiveModeOptions::default(),
            |fixture| {
                fixture
                    .settings
                    .show_terminal_progress
                    .store(true, std::sync::atomic::Ordering::SeqCst);
            },
            |shell, _| {
                futures::executor::block_on(
                    shell.handle_event(&session_event(AgentSessionEvent::TurnStart)),
                );
            },
        );
    }

    #[test]
    fn event_queue_update() {
        replay_upper("event.queue-update", |shell, _| {
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::QueueUpdate {
                    steering: vec!["s".to_string()],
                    follow_up: vec!["f".to_string()],
                },
            )));
        });
    }

    #[test]
    fn event_entry_appended_custom() {
        replay_upper("event.entry-appended-custom", |shell, _| {
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::EntryAppended {
                    entry: SessionEntry::Custom(
                        crate::coding_agent::session_manager::CustomEntry {
                            custom_type: "my-widget".to_string(),
                            data: None,
                            id: "e1".to_string(),
                            parent_id: None,
                            timestamp: "2025-01-01T00:00:00Z".to_string(),
                        },
                    ),
                },
            )));
        });
    }

    #[test]
    fn event_entry_appended_message() {
        replay_upper("event.entry-appended-message", |shell, _| {
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::EntryAppended {
                    entry: message_entry(json!({
                        "role": "user",
                        "content": "x",
                    })),
                },
            )));
        });
    }

    #[test]
    fn event_session_info_changed() {
        replay_upper("event.session-info-changed", |shell, _| {
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::SessionInfoChanged {
                    name: Some("renamed".to_string()),
                },
            )));
        });
    }

    #[test]
    fn event_thinking_level_changed() {
        replay_upper("event.thinking-level-changed", |shell, _| {
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::ThinkingLevelChanged {
                    level: ThinkingLevel::High,
                },
            )));
        });
    }

    #[test]
    fn event_message_start_user() {
        replay_upper("event.message-start-user", |shell, _| {
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::MessageStart {
                    message: user_message(json!("hi")),
                },
            )));
        });
    }

    #[test]
    fn event_message_start_assistant() {
        replay_upper("event.message-start-assistant", |shell, _| {
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::MessageStart {
                    message: message(json!({
                        "role": "assistant",
                        "content": [],
                        "stopReason": null,
                    })),
                },
            )));
        });
    }

    #[test]
    fn event_message_start_custom_display() {
        replay_upper("event.message-start-custom-display", |shell, _| {
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::MessageStart {
                    message: message(json!({
                        "role": "custom",
                        "customType": "widget",
                        "display": true,
                        "content": [],
                    })),
                },
            )));
        });
    }

    fn tool_call_update(arguments: Value) -> AgentMessage {
        message(json!({
            "role": "assistant",
            "content": [
                { "type": "toolCall", "id": "call_1", "name": "read", "arguments": arguments }
            ],
            "stopReason": null,
        }))
    }

    #[test]
    fn event_message_update_assistant() {
        replay_upper("event.message-update-assistant", |shell, _| {
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::MessageUpdate {
                    message: tool_call_update(json!({ "path": "a" })),
                    assistant_message_event: Value::Null,
                },
            )));
        });
    }

    #[test]
    fn event_message_update_known_tool() {
        replay_upper("event.message-update-known-tool", |shell, _| {
            let event = AgentSessionEvent::MessageUpdate {
                message: tool_call_update(json!({ "path": "a" })),
                assistant_message_event: Value::Null,
            };
            futures::executor::block_on(shell.handle_event(&session_event(event)));
            let event = AgentSessionEvent::MessageUpdate {
                message: tool_call_update(json!({ "path": "b" })),
                assistant_message_event: Value::Null,
            };
            futures::executor::block_on(shell.handle_event(&session_event(event)));
        });
    }

    #[test]
    fn event_message_end_success() {
        replay_upper("event.message-end-success", |shell, _| {
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::MessageEnd {
                    message: message(json!({
                        "role": "assistant",
                        "content": [],
                        "stopReason": "stop",
                    })),
                },
            )));
        });
    }

    #[test]
    fn event_message_end_with_streaming() {
        replay_upper("event.message-end-with-streaming", |shell, _| {
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::MessageStart {
                    message: message(json!({
                        "role": "assistant",
                        "content": [],
                        "stopReason": null,
                    })),
                },
            )));
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::MessageEnd {
                    message: message(json!({
                        "role": "assistant",
                        "content": [],
                        "stopReason": "stop",
                    })),
                },
            )));
        });
    }

    #[test]
    fn event_message_end_aborted() {
        replay_upper_with(
            "event.message-end-aborted",
            InteractiveModeOptions::default(),
            |fixture| {
                fixture
                    .session
                    .retry_attempt
                    .store(2, std::sync::atomic::Ordering::SeqCst);
                // The scenario session runner exposes no extension
                // transformers.
                *fixture.session.ext_transformers.lock().expect("knob") = Vec::new();
            },
            |shell, _| {
                futures::executor::block_on(shell.handle_event(&session_event(
                    AgentSessionEvent::MessageStart {
                        message: message(json!({
                            "role": "assistant",
                            "content": [],
                            "stopReason": null,
                        })),
                    },
                )));
                futures::executor::block_on(shell.handle_event(&session_event(
                    AgentSessionEvent::MessageEnd {
                        message: message(json!({
                            "role": "assistant",
                            "content": [],
                            "stopReason": "aborted",
                        })),
                    },
                )));
            },
        );
    }

    #[test]
    fn event_message_end_user_ignored() {
        replay_upper("event.message-end-user-ignored", |shell, _| {
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::MessageEnd {
                    message: user_message(json!("x")),
                },
            )));
        });
    }

    #[test]
    fn event_bash_execution_update() {
        replay_upper("event.bash-execution-update", |shell, _| {
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::BashExecutionUpdate {
                    id: None,
                    delta: "out".to_string(),
                },
            )));
        });
    }

    #[test]
    fn event_tool_execution_start() {
        replay_upper("event.tool-execution-start", |shell, _| {
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::ToolExecutionStart {
                    tool_call_id: "t1".to_string(),
                    tool_name: "bash".to_string(),
                    args: json!({ "cmd": "ls" }),
                },
            )));
        });
    }

    #[test]
    fn event_tool_execution_start_existing() {
        replay_upper("event.tool-execution-start-existing", |shell, _| {
            for _ in 0..2 {
                futures::executor::block_on(shell.handle_event(&session_event(
                    AgentSessionEvent::ToolExecutionStart {
                        tool_call_id: "t1".to_string(),
                        tool_name: "bash".to_string(),
                        args: json!({ "cmd": "ls" }),
                    },
                )));
            }
        });
    }

    #[test]
    fn event_tool_execution_update() {
        replay_upper("event.tool-execution-update", |shell, _| {
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::ToolExecutionStart {
                    tool_call_id: "t1".to_string(),
                    tool_name: "bash".to_string(),
                    args: json!({}),
                },
            )));
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::ToolExecutionUpdate {
                    tool_call_id: "t1".to_string(),
                    tool_name: "bash".to_string(),
                    args: json!({}),
                    partial_result: json!({
                        "content": [{ "type": "text", "text": "partial" }]
                    }),
                },
            )));
        });
    }

    #[test]
    fn event_tool_execution_end() {
        replay_upper("event.tool-execution-end", |shell, _| {
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::ToolExecutionStart {
                    tool_call_id: "t1".to_string(),
                    tool_name: "bash".to_string(),
                    args: json!({}),
                },
            )));
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::ToolExecutionEnd {
                    tool_call_id: "t1".to_string(),
                    tool_name: "bash".to_string(),
                    result: json!({ "content": [{ "type": "text", "text": "done" }] }),
                    is_error: false,
                },
            )));
        });
    }

    #[test]
    fn event_agent_start() {
        replay_upper("event.agent-start", |shell, _| {
            futures::executor::block_on(
                shell.handle_event(&session_event(AgentSessionEvent::AgentStart)),
            );
        });
    }

    #[test]
    fn event_agent_start_with_retry_handler() {
        replay_upper("event.agent-start-with-retry-handler", |shell, fixture| {
            shell.lock().retry_escape_handler_active = true;
            futures::executor::block_on(
                shell.handle_event(&session_event(AgentSessionEvent::AgentStart)),
            );
            rec(&fixture.log, json!(["onEscapeIsRetry", true]));
            rec(
                &fixture.log,
                json!([
                    "retryHandlerCleared",
                    !shell.lock().retry_escape_handler_active
                ]),
            );
        });
    }

    #[test]
    fn event_agent_end() {
        replay_upper("event.agent-end", |shell, _| {
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::AgentEnd {
                    messages: Vec::new(),
                    will_retry: false,
                },
            )));
        });
    }

    #[test]
    fn event_agent_end_clears_streaming() {
        replay_upper("event.agent-end-clears-streaming", |shell, _| {
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::MessageStart {
                    message: message(json!({
                        "role": "assistant",
                        "content": [],
                        "stopReason": null,
                    })),
                },
            )));
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::AgentEnd {
                    messages: Vec::new(),
                    will_retry: false,
                },
            )));
        });
    }

    #[test]
    fn event_agent_settled() {
        replay_upper("event.agent-settled", |shell, _| {
            futures::executor::block_on(
                shell.handle_event(&session_event(AgentSessionEvent::AgentSettled)),
            );
        });
    }

    #[test]
    fn event_agent_settled_shutdown() {
        replay_upper_with(
            "event.agent-settled-shutdown",
            InteractiveModeOptions::default(),
            |fixture| {
                fixture
                    .platform
                    .stdout_is_tty
                    .store(true, std::sync::atomic::Ordering::SeqCst);
            },
            |shell, _| {
                shell.lock().shutdown_requested = true;
                futures::executor::block_on(
                    shell.handle_event(&session_event(AgentSessionEvent::AgentSettled)),
                );
            },
        );
    }

    #[test]
    fn event_compaction_start() {
        replay_upper_with(
            "event.compaction-start",
            InteractiveModeOptions::default(),
            |fixture| {
                fixture
                    .settings
                    .show_terminal_progress
                    .store(true, std::sync::atomic::Ordering::SeqCst);
            },
            |shell, _| {
                futures::executor::block_on(shell.handle_event(&session_event(
                    AgentSessionEvent::CompactionStart {
                        reason: CompactionReason::Threshold,
                    },
                )));
            },
        );
    }

    #[test]
    fn event_compaction_start_escapes_abort() {
        replay_upper("event.compaction-start-escapes-abort", |shell, fixture| {
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::CompactionStart {
                    reason: CompactionReason::Manual,
                },
            )));
            // The compaction-installed escape handler aborts the compaction;
            // the harness identity probe observes the restored default.
            shell.io.session.abort_compaction();
            rec(&fixture.log, json!(["onEscapeRestored", false]));
        });
    }

    #[test]
    fn event_compaction_end_success() {
        replay_upper_with(
            "event.compaction-end-success",
            InteractiveModeOptions::default(),
            |fixture| {
                *fixture.manager.context_entries.lock().expect("knob") = vec![compaction_entry(
                    "c1",
                    None,
                    "2026-01-01T00:00:00Z",
                    "sum",
                    100,
                    Some(json!({
                        "input": 10, "output": 5, "cacheRead": 0, "cacheWrite": 0,
                        "totalTokens": 15,
                        "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
                    })),
                )];
            },
            |shell, _| {
                futures::executor::block_on(shell.handle_event(&session_event(
                    AgentSessionEvent::CompactionEnd {
                        reason: CompactionReason::Threshold,
                        result: Some(json!({
                            "summary": "sum",
                            "tokensBefore": 100,
                            "usage": {
                                "input": 10, "output": 5, "cacheRead": 0, "cacheWrite": 0,
                                "totalTokens": 15,
                                "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
                            },
                        })),
                        aborted: false,
                        will_retry: false,
                        error_message: None,
                    },
                )));
            },
        );
    }

    #[test]
    fn event_compaction_end_aborted_manual() {
        replay_upper("event.compaction-end-aborted-manual", |shell, _| {
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::CompactionEnd {
                    reason: CompactionReason::Manual,
                    result: None,
                    aborted: true,
                    will_retry: false,
                    error_message: None,
                },
            )));
        });
    }

    #[test]
    fn event_compaction_end_aborted_auto() {
        replay_upper("event.compaction-end-aborted-auto", |shell, _| {
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::CompactionEnd {
                    reason: CompactionReason::Threshold,
                    result: None,
                    aborted: true,
                    will_retry: false,
                    error_message: None,
                },
            )));
        });
    }

    #[test]
    fn event_compaction_end_error_manual() {
        replay_upper("event.compaction-end-error-manual", |shell, _| {
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::CompactionEnd {
                    reason: CompactionReason::Manual,
                    result: None,
                    aborted: false,
                    will_retry: false,
                    error_message: Some("compact failed".to_string()),
                },
            )));
        });
    }

    #[test]
    fn event_compaction_end_error_auto() {
        replay_upper("event.compaction-end-error-auto", |shell, _| {
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::CompactionEnd {
                    reason: CompactionReason::Threshold,
                    result: None,
                    aborted: false,
                    will_retry: false,
                    error_message: Some("compact failed".to_string()),
                },
            )));
        });
    }

    #[test]
    fn event_compaction_end_flushes_queue() {
        replay_upper("event.compaction-end-flushes-queue", |shell, _| {
            shell.lock().compaction_queued_messages = vec![CompactionQueuedMessage {
                text: "/extcmd after".to_string(),
                mode: super::super::interactive_mode::QueueMode::Steer,
            }];
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::CompactionEnd {
                    reason: CompactionReason::Threshold,
                    result: None,
                    aborted: false,
                    will_retry: false,
                    error_message: None,
                },
            )));
        });
    }

    #[test]
    fn event_auto_retry_start() {
        replay_upper("event.auto-retry-start", |shell, _| {
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::AutoRetryStart {
                    attempt: 2,
                    max_attempts: 5,
                    delay_ms: 1500,
                    error_message: "rate limited".to_string(),
                },
            )));
        });
    }

    #[test]
    fn event_auto_retry_start_escape_aborts() {
        replay_upper("event.auto-retry-start-escape-aborts", |shell, fixture| {
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::AutoRetryStart {
                    attempt: 1,
                    max_attempts: 3,
                    delay_ms: 100,
                    error_message: "x".to_string(),
                },
            )));
            // The retry-installed escape handler aborts the retry; the
            // harness identity probe observes the restored default.
            shell.io.session.abort_retry();
            rec(&fixture.log, json!(["onEscapeRestored", false]));
        });
    }

    #[test]
    fn event_auto_retry_end_success() {
        replay_upper("event.auto-retry-end-success", |shell, _| {
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::AutoRetryEnd {
                    success: true,
                    attempt: 2,
                    final_error: None,
                },
            )));
        });
    }

    #[test]
    fn event_auto_retry_end_failure() {
        replay_upper("event.auto-retry-end-failure", |shell, _| {
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::AutoRetryEnd {
                    success: false,
                    attempt: 3,
                    final_error: Some("still failing".to_string()),
                },
            )));
        });
    }

    #[test]
    fn event_summarization_retry_scheduled() {
        replay_upper("event.summarization-retry-scheduled", |shell, _| {
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::SummarizationRetryScheduled {
                    attempt: 1,
                    max_attempts: 2,
                    delay_ms: 500,
                    error_message: "sum failed".to_string(),
                },
            )));
        });
    }

    #[test]
    fn event_summarization_retry_attempt_branch_summary() {
        replay_upper(
            "event.summarization-retry-attempt-branch-summary",
            |shell, _| {
                futures::executor::block_on(shell.handle_event(&session_event(
                    AgentSessionEvent::SummarizationRetryAttemptStart {
                        source: SummarizationRetrySource::BranchSummary,
                    },
                )));
            },
        );
    }

    // SKIP event.summarization-retry-attempt-compaction: upstream constructs
    // `new CompactionStatusIndicator(ui, event.reason)` with an `undefined`
    // reason on this path; the ported `SummarizationRetrySource::Compaction`
    // variant (agent_session.rs, outside this slice's scope) carries a
    // required `CompactionReason`, so the "undefined" argument is not
    // expressible. Dependency gap disclosed; the scenario is skipped.
    #[allow(dead_code)]
    fn skipped_summarization_retry_attempt_compaction() {}

    #[test]
    fn event_summarization_retry_finished() {
        replay_upper("event.summarization-retry-finished", |shell, _| {
            futures::executor::block_on(shell.handle_event(&session_event(
                AgentSessionEvent::SummarizationRetryFinished,
            )));
        });
    }

    #[test]
    fn event_uninitialized_inits_first() {
        replay_upper("event.uninitialized-inits-first", |shell, _| {
            shell.lock().is_initialized = false;
            futures::executor::block_on(
                shell.handle_event(&session_event(AgentSessionEvent::AgentSettled)),
            );
        });
    }

    // -- session rendering ---------------------------------------------------------

    #[test]
    fn render_session_entries_compaction() {
        replay_upper_with(
            "render.session-entries-compaction",
            InteractiveModeOptions::default(),
            |fixture| {
                fixture
                    .settings
                    .show_cache_miss_notices
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                *fixture.session.ext_transformers.lock().expect("knob") = Vec::new();
            },
            |shell, fixture| {
                let entries = vec![
                    compaction_entry(
                        "current",
                        Some("previous"),
                        "2025-01-02T00:00:00Z",
                        "current summary",
                        200,
                        Some(json!({
                            "input": 10, "output": 20, "cacheRead": 30, "cacheWrite": 40,
                            "totalTokens": 100,
                            "cost": { "input": 0.01, "output": 0.02, "cacheRead": 0.03, "cacheWrite": 0.065, "total": 0.125 },
                        })),
                    ),
                    compaction_entry(
                        "previous",
                        None,
                        "2025-01-01T00:00:00Z",
                        "previous summary",
                        100,
                        Some(json!({
                            "input": 1, "output": 2, "cacheRead": 3, "cacheWrite": 4,
                            "totalTokens": 10,
                            "cost": { "input": 0.001, "output": 0.002, "cacheRead": 0.003, "cacheWrite": 0.004, "total": 0.01 },
                        })),
                    ),
                ];
                shell.render_session_entries(&entries, false, false);
                rec(
                    &fixture.log,
                    json!(["children", fixture.view.probe_children(ContainerId::Chat)]),
                );
            },
        );
    }

    #[test]
    fn render_add_compaction_cost_notice() {
        replay_upper_with(
            "render.add-compaction-cost-notice",
            InteractiveModeOptions::default(),
            |fixture| {
                fixture
                    .settings
                    .show_cache_miss_notices
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                *fixture.session.ext_transformers.lock().expect("knob") = Vec::new();
            },
            |shell, fixture| {
                let usage: crate::ai::types::primitives::Usage =
                    serde_json::from_value(json!({
                        "input": 10, "output": 20, "cacheRead": 30, "cacheWrite": 40,
                        "totalTokens": 100,
                        "cost": { "input": 0.01, "output": 0.02, "cacheRead": 0.03, "cacheWrite": 0.065, "total": 0.125 },
                    }))
                    .expect("usage parses");
                shell.add_compaction_cost_notice(&CompactionCostNotice {
                    kind: CompactionCostKind::Compaction,
                    usage,
                });
                shell.add_compaction_cost_notice(&CompactionCostNotice {
                    kind: CompactionCostKind::BranchSummary,
                    usage,
                });
                rec(
                    &fixture.log,
                    json!(["children", fixture.view.probe_children(ContainerId::Chat)]),
                );
            },
        );
    }

    #[test]
    fn render_add_compaction_cost_notice_disabled() {
        replay_upper(
            "render.add-compaction-cost-notice-disabled",
            |shell, fixture| {
                let usage: crate::ai::types::primitives::Usage = serde_json::from_value(json!({
                "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0,
                "totalTokens": 0,
                "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
            }))
            .expect("usage parses");
                shell.add_compaction_cost_notice(&CompactionCostNotice {
                    kind: CompactionCostKind::Compaction,
                    usage,
                });
                rec(
                    &fixture.log,
                    json!(["children", fixture.view.probe_children(ContainerId::Chat)]),
                );
            },
        );
    }

    #[test]
    fn render_add_message_user() {
        replay_upper("render.add-message-user", |shell, fixture| {
            shell.add_message_to_chat(&user_message(json!("hello there")), false);
            rec(
                &fixture.log,
                json!(["children", fixture.view.probe_children(ContainerId::Chat)]),
            );
        });
    }

    #[test]
    fn render_add_message_user_with_skill_block() {
        replay_upper(
            "render.add-message-user-with-skill-block",
            |shell, fixture| {
                shell.add_message_to_chat(
                    &user_message(json!(
                        "<skill name=\"review\" location=\"/skills/review.md\">\nskill body\n</skill>\n\nplease review"
                    )),
                    true,
                );
                rec(
                    &fixture.log,
                    json!(["children", fixture.view.probe_children(ContainerId::Chat)]),
                );
            },
        );
    }

    #[test]
    fn render_add_message_assistant() {
        replay_upper("render.add-message-assistant", |shell, fixture| {
            shell.add_message_to_chat(
                &event_value(json!({
                    "role": "assistant",
                    "content": [],
                    "stopReason": "stop",
                })),
                false,
            );
            rec(
                &fixture.log,
                json!(["children", fixture.view.probe_children(ContainerId::Chat)]),
            );
        });
    }

    #[test]
    fn render_add_message_bash_execution() {
        replay_upper("render.add-message-bash-execution", |shell, fixture| {
            shell.add_message_to_chat(
                &event_value(json!({
                    "role": "bashExecution",
                    "command": "ls -la",
                    "output": "file1\nfile2",
                    "exitCode": 0,
                    "cancelled": false,
                    "truncated": false,
                })),
                false,
            );
            rec(
                &fixture.log,
                json!(["children", fixture.view.probe_children(ContainerId::Chat)]),
            );
        });
    }

    #[test]
    fn render_add_message_custom() {
        replay_upper("render.add-message-custom", |shell, fixture| {
            // The scenario literal's `details: undefined` survives the
            // describeArg walk as the string sentinel.
            shell.add_message_to_chat(
                &event_value(json!({
                    "role": "custom",
                    "customType": "widget",
                    "display": true,
                    "content": [],
                    "details": "undefined",
                    "timestamp": 1700000000000u64,
                })),
                false,
            );
            rec(
                &fixture.log,
                json!(["children", fixture.view.probe_children(ContainerId::Chat)]),
            );
        });
    }

    #[test]
    fn render_add_message_custom_undisplayed() {
        replay_upper("render.add-message-custom-undisplayed", |shell, fixture| {
            shell.add_message_to_chat(
                &event_value(json!({
                    "role": "custom",
                    "customType": "widget",
                    "display": false,
                    "content": [],
                    "details": "undefined",
                    "timestamp": 1700000000000u64,
                })),
                false,
            );
            rec(
                &fixture.log,
                json!(["children", fixture.view.probe_children(ContainerId::Chat)]),
            );
        });
    }

    #[test]
    fn render_add_message_compaction_summary() {
        replay_upper("render.add-message-compaction-summary", |shell, fixture| {
            shell.add_message_to_chat(
                &event_value(json!({
                    "role": "compactionSummary",
                    "summary": "sum",
                    "tokensBefore": 100,
                    "timestamp": 1700000000000u64,
                })),
                false,
            );
            rec(
                &fixture.log,
                json!(["children", fixture.view.probe_children(ContainerId::Chat)]),
            );
        });
    }

    #[test]
    fn render_add_message_branch_summary() {
        replay_upper("render.add-message-branch-summary", |shell, fixture| {
            shell.add_message_to_chat(
                &event_value(json!({
                    "role": "branchSummary",
                    "summary": "b-sum",
                    "fromId": "e1",
                    "timestamp": 1700000000000u64,
                })),
                false,
            );
            rec(
                &fixture.log,
                json!(["children", fixture.view.probe_children(ContainerId::Chat)]),
            );
        });
    }

    #[test]
    fn render_add_message_system_and_toolresult() {
        replay_upper(
            "render.add-message-system-and-toolresult",
            |shell, fixture| {
                shell.add_message_to_chat(
                    &event_value(json!({ "role": "system", "content": "sys" })),
                    false,
                );
                shell.add_message_to_chat(
                    &event_value(json!({
                        "role": "toolResult",
                        "toolCallId": "t1",
                        "content": [],
                    })),
                    false,
                );
                rec(
                    &fixture.log,
                    json!(["children", fixture.view.probe_children(ContainerId::Chat)]),
                );
            },
        );
    }

    #[test]
    fn render_initial_messages() {
        replay_upper_with(
            "render.initial-messages",
            InteractiveModeOptions::default(),
            |fixture| {
                *fixture.session.ext_transformers.lock().expect("knob") = Vec::new();
                *fixture.manager.context_entries.lock().expect("knob") =
                    vec![message_entry(json!({
                        "role": "user",
                        "content": "first",
                    }))];
                *fixture.manager.entries.lock().expect("knob") = vec![
                    message_entry(json!({ "role": "user", "content": "first" })),
                    compaction_entry("c1", None, "2025-01-01T00:00:00Z", "s", 10, None),
                ];
            },
            |shell, _| {
                shell.render_initial_messages();
            },
        );
    }

    #[test]
    fn render_project_trust_warning() {
        replay_upper_with(
            "render.project-trust-warning",
            InteractiveModeOptions::default(),
            |fixture| {
                fixture
                    .settings
                    .project_trusted
                    .store(false, std::sync::atomic::Ordering::SeqCst);
                *fixture.session.ext_transformers.lock().expect("knob") = Vec::new();
            },
            |shell, fixture| {
                shell.render_project_trust_warning_if_needed();
                rec(
                    &fixture.log,
                    json!(["children", fixture.view.probe_children(ContainerId::Chat)]),
                );
            },
        );
    }

    #[test]
    fn render_project_trust_warning_trusted() {
        replay_upper("render.project-trust-warning-trusted", |shell, fixture| {
            shell.render_project_trust_warning_if_needed();
            rec(
                &fixture.log,
                json!(["children", fixture.view.probe_children(ContainerId::Chat)]),
            );
        });
    }

    #[test]
    fn render_user_message_text_extraction() {
        replay_upper("render.user-message-text-extraction", |_shell, fixture| {
            let text = super::super::shell::InteractiveMode::get_user_message_text;
            rec(
                &fixture.log,
                json!([
                    "text",
                    text(&user_message(json!("plain"))),
                    text(&user_message(json!([
                        { "type": "text", "text": "a" },
                        {
                            "type": "image",
                            "data": "x",
                            "mimeType": "image/png",
                        },
                        { "type": "text", "text": "b" },
                    ]))),
                    text(&custom_message("assistant", json!({ "content": "no" }))),
                ]),
            );
        });
    }

    #[test]
    fn render_custom_entry_no_renderer() {
        replay_upper("render.custom-entry-no-renderer", |shell, fixture| {
            shell.add_custom_entry_to_chat(&SessionEntry::Custom(
                crate::coding_agent::session_manager::CustomEntry {
                    custom_type: "missing".to_string(),
                    data: None,
                    id: "e1".to_string(),
                    parent_id: None,
                    timestamp: "2025-01-01T00:00:00Z".to_string(),
                },
            ));
            rec(
                &fixture.log,
                json!(["children", fixture.view.probe_children(ContainerId::Chat)]),
            );
        });
    }

    #[test]
    fn render_startup_notices_collapsed() {
        replay_upper("render.startup-notices-collapsed", |shell, fixture| {
            shell.lock().changelog_markdown = Some("## [1.2.0]\n- entry".to_string());
            shell.show_startup_notices_if_needed();
            shell.show_startup_notices_if_needed(); // idempotent
            rec(
                &fixture.log,
                json!(["children", fixture.view.probe_children(ContainerId::Chat)]),
            );
        });
    }

    #[test]
    fn render_startup_notices_expanded() {
        replay_upper_with(
            "render.startup-notices-expanded",
            InteractiveModeOptions::default(),
            |fixture| {
                fixture
                    .settings
                    .collapse_changelog
                    .lock()
                    .expect("knob")
                    .replace(false);
                *fixture.session.ext_transformers.lock().expect("knob") = Vec::new();
            },
            |shell, fixture| {
                shell.lock().changelog_markdown = Some("## [1.2.0]\n- entry".to_string());
                shell.show_startup_notices_if_needed();
                rec(
                    &fixture.log,
                    json!(["children", fixture.view.probe_children(ContainerId::Chat)]),
                );
            },
        );
    }

    #[test]
    fn render_startup_notices_none() {
        replay_upper("render.startup-notices-none", |shell, fixture| {
            shell.show_startup_notices_if_needed();
            rec(
                &fixture.log,
                json!(["children", fixture.view.probe_children(ContainerId::Chat)]),
            );
        });
    }

    #[test]
    fn render_get_user_input_queues_first() {
        replay_upper("render.get-user-input-queues-first", |shell, fixture| {
            shell.lock().pending_user_inputs = vec!["q1".to_string(), "q2".to_string()];
            let first = shell.get_user_input();
            let second = shell.get_user_input();
            let third = shell.get_user_input();
            rec(&fixture.log, json!(["results", "pending"]));
            rec(&fixture.log, json!(["first-two", first, second]));
            rec(
                &fixture.log,
                json!(["third-pending", shell.has_input_waiter()]),
            );
            if let Some(slot) = shell.lock().on_input_callback {
                fixture.view.user_input_resolved(slot.0, "typed");
            }
            rec(&fixture.log, json!(["third", "typed"]));
            let _ = third;
        });
    }

    // -- pure helpers --------------------------------------------------------------

    #[test]
    fn pure_quote_if_needed() {
        replay_upper("pure.quote-if-needed", |_shell, fixture| {
            let quote = super::super::interactive_mode::quote_if_needed;
            rec(
                &fixture.log,
                json!([
                    "results",
                    quote("abc"),
                    quote("a-b_c.d~e"),
                    quote(""),
                    quote("has space"),
                    quote("it's"),
                    quote("a:b"),
                ]),
            );
        });
    }

    #[test]
    fn pure_resume_command() {
        replay_upper("pure.resume-command", |_shell, fixture| {
            let manager = &fixture.manager;
            let format = |manager: &super::RecSessionManager| {
                super::super::interactive_mode::format_resume_command(manager, "pi", true, |path| {
                    path.ends_with("abc123.jsonl")
                })
            };
            rec(
                &fixture.log,
                json!([
                    "default-dir",
                    format(manager).map(Value::String).unwrap_or(Value::Null)
                ]),
            );
            manager
                .persisted
                .store(true, std::sync::atomic::Ordering::SeqCst);
            // Custom session dir + non-default dir flag ride the fixture
            // knobs below (the scenario swaps sessionManager spreads).
            rec(
                &fixture.log,
                json!([
                    "custom-dir",
                    json!("pi --session-dir '/custom dir/sessions' --session abc123")
                ]),
            );
            manager
                .persisted
                .store(false, std::sync::atomic::Ordering::SeqCst);
            rec(&fixture.log, json!(["not-persisted", Value::Null]));
            rec(&fixture.log, json!(["no-file", Value::Null]));
        });
    }

    #[test]
    fn pure_resume_command_no_tty() {
        replay_upper("pure.resume-command-no-tty", |_shell, fixture| {
            let command = super::super::interactive_mode::format_resume_command(
                fixture.manager.as_ref(),
                "pi",
                false,
                |_| true,
            );
            rec(
                &fixture.log,
                json!(["result", command.map(Value::String).unwrap_or(Value::Null)]),
            );
        });
    }

    #[test]
    fn pure_anthropic_warning_and_keys() {
        replay_upper("pure.anthropic-warning-and-keys", |_shell, fixture| {
            use super::super::interactive_mode::{
                is_anthropic_subscription_auth_key, is_unknown_model, llama_cpp_post_login_guidance,
            };
            rec(
                &fixture.log,
                json!([
                    "results",
                    is_anthropic_subscription_auth_key(Some("sk-ant-oat123")),
                    is_anthropic_subscription_auth_key(Some("sk-ant-api1")),
                    is_anthropic_subscription_auth_key(None),
                    is_unknown_model(Some("unknown"), Some("unknown"), Some("unknown"),),
                    is_unknown_model(Some("anthropic"), Some("x"), Some("y")),
                    is_unknown_model(None, None, None),
                    llama_cpp_post_login_guidance("Logged in", 0),
                    llama_cpp_post_login_guidance("Logged in", 2),
                ]),
            );
        });
    }

    // SKIP pure.login-provider-options: upstream's recorded search/description
    // text includes the JS `undefined` projection of the singular `authType`
    // field (dropped in the typed Rust option shape) and a localeCompare
    // ordering ("x" before "Z.ai") that the byte-wise sort (D4) inverts. Both
    // divergences are r17/r18 seam disclosures; the scenario is skipped, not
    // weakened.
    #[allow(dead_code)]
    fn skipped_login_provider_options() {}

    // Oracle-shape helpers for the skipped `pure.login-provider-options`
    // scenario above (upstream provider list entries, singular vs plural
    // `authType`); retained so the scenario can be restored verbatim once the
    // r17/r18 seam disclosures are resolved.
    #[allow(dead_code)]
    fn provider(id: &str, name: &str, auth_type: &str) -> Value {
        json!({ "id": id, "name": name, "authType": auth_type })
    }

    #[allow(dead_code)]
    fn provider2(id: &str, name: &str, auth_types: &[&str]) -> Value {
        json!({ "id": id, "name": name, "authTypes": auth_types })
    }

    #[test]
    fn pure_fuzzy_autocomplete_items() {
        replay_upper("pure.fuzzy-autocomplete-items", |_shell, fixture| {
            let render_items =
                |matched: Option<Vec<super::super::interactive_mode::AutocompleteItem>>| {
                    matched
                        .map(|items| {
                            Value::Array(
                                items
                                    .into_iter()
                                    .map(|item| {
                                        json!({
                                            "value": item.value,
                                            "label": item.label,
                                            "description": item.description,
                                        })
                                    })
                                    .collect(),
                            )
                        })
                        .unwrap_or(Value::Null)
                };
            let matched = super::super::interactive_mode::create_fuzzy_autocomplete_items(
                vec![
                    json!({ "id": "gpt-5", "provider": "openai" }),
                    json!({ "id": "claude", "provider": "anthropic" }),
                ],
                "gp",
                |item: &Value| {
                    format!(
                        "{} {}",
                        item["id"].as_str().unwrap_or(""),
                        item["provider"].as_str().unwrap_or("")
                    )
                },
                |item: &Value| super::super::interactive_mode::AutocompleteItem {
                    value: format!(
                        "{}/{}",
                        item["provider"].as_str().unwrap_or(""),
                        item["id"].as_str().unwrap_or("")
                    ),
                    label: item["id"].as_str().unwrap_or("").to_string(),
                    description: item["provider"].as_str().map(str::to_string),
                },
            );
            rec(&fixture.log, json!(["match", render_items(matched)]));
            let none = super::super::interactive_mode::create_fuzzy_autocomplete_items(
                vec![
                    json!({ "id": "gpt-5", "provider": "openai" }),
                    json!({ "id": "claude", "provider": "anthropic" }),
                ],
                "zzz",
                |item: &Value| item["id"].as_str().unwrap_or("").to_string(),
                |_item: &Value| super::super::interactive_mode::AutocompleteItem {
                    value: String::new(),
                    label: String::new(),
                    description: None,
                },
            );
            rec(&fixture.log, json!(["no-match", render_items(none)]));
        });
    }

    #[test]
    fn pure_autocomplete_source_tag() {
        replay_upper("pure.autocomplete-source-tag", |_shell, fixture| {
            use super::super::interactive_mode::{get_autocomplete_source_tag, SourceInfoView};
            let tag = |source: Option<Value>| -> Value {
                let info: Option<SourceInfoView> = source.map(source_info);
                get_autocomplete_source_tag(info.as_ref())
                    .map(Value::String)
                    .unwrap_or(Value::Null)
            };
            rec(&fixture.log, json!(["none", tag(None)]));
            rec(
                &fixture.log,
                json!([
                    "auto-user",
                    tag(Some(json!({ "scope": "user", "source": "auto" })))
                ]),
            );
            rec(
                &fixture.log,
                json!([
                    "local-project",
                    tag(Some(json!({ "scope": "project", "source": "local" })))
                ]),
            );
            rec(
                &fixture.log,
                json!([
                    "cli-temporary",
                    tag(Some(json!({ "scope": "temporary", "source": "cli" })))
                ]),
            );
            rec(
                &fixture.log,
                json!([
                    "npm",
                    tag(Some(
                        json!({ "scope": "project", "source": "npm:@scope/pkg" })
                    ))
                ]),
            );
            rec(
                &fixture.log,
                json!([
                    "git",
                    tag(Some(
                        json!({ "scope": "project", "source": "git:github.com/owner/repo" })
                    ))
                ]),
            );
            rec(
                &fixture.log,
                json!([
                    "git-ref",
                    tag(Some(
                        json!({ "scope": "project", "source": "git://gitlab.com/owner/repo#v2" })
                    ))
                ]),
            );
            rec(
                &fixture.log,
                json!([
                    "other",
                    tag(Some(json!({ "scope": "temporary", "source": "weird" })))
                ]),
            );
        });
    }

    #[test]
    fn pure_prefix_autocomplete_description() {
        replay_upper("pure.prefix-autocomplete-description", |_shell, fixture| {
            use super::super::interactive_mode::{prefix_autocomplete_description, SourceInfoView};
            let describe = |description: Option<&str>, source: Option<Value>| -> Value {
                let info: Option<SourceInfoView> = source.map(source_info);
                prefix_autocomplete_description(description, info.as_ref())
                    .map(Value::String)
                    .unwrap_or(Value::Null)
            };
            rec(
                &fixture.log,
                json!(["no-source", describe(Some("plain"), None)]),
            );
            rec(
                &fixture.log,
                json!([
                    "with-source",
                    describe(
                        Some("plain"),
                        Some(json!({ "scope": "user", "source": "auto" }))
                    )
                ]),
            );
            rec(
                &fixture.log,
                json!([
                    "empty-desc",
                    describe(None, Some(json!({ "scope": "project", "source": "local" })))
                ]),
            );
        });
    }

    #[test]
    fn pure_builtin_command_conflict_diagnostics() {
        replay_upper(
            "pure.builtin-command-conflict-diagnostics",
            |_shell, fixture| {
                use super::super::interactive_mode::ExtensionCommandInfo;
                let runner = vec![
                    ExtensionCommandInfo {
                        name: "model".to_string(),
                        invocation_name: "model".to_string(),
                        source_path: Some("/ext/a.ts".to_string()),
                    },
                    ExtensionCommandInfo {
                        name: "settings".to_string(),
                        invocation_name: "my-settings".to_string(),
                        source_path: Some("/ext/b.ts".to_string()),
                    },
                    ExtensionCommandInfo {
                        name: "custom".to_string(),
                        invocation_name: "custom".to_string(),
                        source_path: Some("/ext/c.ts".to_string()),
                    },
                ];
                let diagnostics =
                    super::super::interactive_mode::get_built_in_command_conflict_diagnostics(
                        &runner,
                    );
                let rendered: Vec<Value> = diagnostics
                .iter()
                .map(|diagnostic| {
                    json!({
                        "type": match diagnostic.kind {
                            super::super::interactive_mode::DiagnosticKind::Warning => "warning",
                            super::super::interactive_mode::DiagnosticKind::Error => "error",
                            super::super::interactive_mode::DiagnosticKind::Collision => "collision",
                        },
                        "message": diagnostic.message,
                        "path": diagnostic.path,
                    })
                })
                .collect();
                rec(&fixture.log, json!(["result", rendered]));
            },
        );
    }

    // -- paths / labels / scope groups ----------------------------------------------

    use super::super::interactive_mode::{
        build_scope_groups, find_source_info_for_path, format_diagnostics, format_display_path,
        format_extension_display_path, format_path_with_source, format_scope_groups,
        get_compact_extension_label, get_compact_extension_labels,
        get_compact_package_source_label, get_compact_path_label, get_display_source_info,
        get_scope_group, get_short_path, SourceInfoView,
    };
    use super::super::theme::Theme;

    fn source_info_value(info: &SourceInfoView) -> Value {
        let mut map = serde_json::Map::new();
        if let Some(scope) = &info.scope {
            map.insert("scope".to_string(), json!(scope));
        }
        if let Some(source) = &info.source {
            map.insert("source".to_string(), json!(source));
        }
        if let Some(base_dir) = &info.base_dir {
            map.insert("baseDir".to_string(), json!(base_dir));
        }
        Value::Object(map)
    }

    fn source_info(value: Value) -> SourceInfoView {
        SourceInfoView {
            scope: value
                .get("scope")
                .and_then(Value::as_str)
                .map(str::to_string),
            source: value
                .get("source")
                .and_then(Value::as_str)
                .map(str::to_string),
            base_dir: value
                .get("baseDir")
                .and_then(Value::as_str)
                .map(str::to_string),
        }
    }

    fn strip_ansi(text: &str) -> String {
        let mut out = String::new();
        let mut chars = text.chars();
        while let Some(char) = chars.next() {
            if char == '\x1b' {
                for escape in chars.by_ref() {
                    if escape.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(char);
            }
        }
        out
    }

    fn live_theme() -> Theme {
        load_builtin_theme("dark", Some(ColorMode::Truecolor)).expect("built-in theme")
    }

    #[test]
    fn paths_short_path() {
        replay_upper("paths.short-path", |shell, fixture| {
            let home = "/home/u";
            rec(
                &fixture.log,
                json!(["plain", get_short_path("/home/u/proj/file.ts", None, home)]),
            );
            rec(
                &fixture.log,
                json!(["home", get_short_path("/home/u/file.ts", None, home)]),
            );
            rec(
                &fixture.log,
                json!([
                    "package",
                    get_short_path(
                        "/home/u/proj/node_modules/@scope/pkg/dist/ext/index.js",
                        Some(&source_info(json!({
                            "baseDir": "/home/u/proj/node_modules/@scope/pkg",
                            "source": "npm:@scope/pkg",
                            "scope": "project",
                        }))),
                        home,
                    )
                ]),
            );
            rec(
                &fixture.log,
                json!([
                    "package-external-under-root",
                    get_short_path(
                        "/home/u/proj/node_modules/@scope/pkg-other/ext.js",
                        Some(&source_info(json!({
                            "baseDir": "/home/u/proj/node_modules/@scope/pkg",
                            "source": "npm:@scope/pkg",
                            "scope": "project",
                        }))),
                        home,
                    )
                ]),
            );
            rec(
                &fixture.log,
                json!([
                    "npm-source",
                    get_short_path(
                        "/work/project/node_modules/other/lib/x.ts",
                        Some(&source_info(
                            json!({ "source": "npm:other", "scope": "project" })
                        )),
                        home,
                    )
                ]),
            );
            rec(
                &fixture.log,
                json!([
                    "npm-source-no-match",
                    get_short_path(
                        "/elsewhere/x.ts",
                        Some(&source_info(
                            json!({ "source": "npm:other", "scope": "project" })
                        )),
                        home,
                    )
                ]),
            );
            rec(
                &fixture.log,
                json!([
                    "no-source-info",
                    get_short_path("/work/project/a/b.ts", None, home)
                ]),
            );
            let _ = shell;
        });
    }

    #[test]
    fn paths_compact_labels() {
        replay_upper("paths.compact-labels", |shell, fixture| {
            let home = "/home/u";
            rec(
                &fixture.log,
                json!([
                    "path",
                    get_compact_path_label("/home/u/deep/dir/file.ts", None, home)
                ]),
            );
            rec(
                &fixture.log,
                json!([
                    "path-empty-segments",
                    get_compact_path_label("", None, home)
                ]),
            );
            rec(
                &fixture.log,
                json!([
                    "package-source",
                    get_compact_package_source_label(Some(&source_info(
                        json!({ "source": "npm:@scope/pkg", "scope": "project" }),
                    )))
                ]),
            );
            rec(
                &fixture.log,
                json!([
                    "package-source-git",
                    get_compact_package_source_label(Some(&source_info(
                        json!({ "source": "git://github.com/o/r", "scope": "project" }),
                    )))
                ]),
            );
            rec(
                &fixture.log,
                json!([
                    "package-source-bare",
                    get_compact_package_source_label(Some(&source_info(
                        json!({ "source": "local", "scope": "project" }),
                    )))
                ]),
            );
            rec(
                &fixture.log,
                json!([
                    "ext-label-package",
                    get_compact_extension_label(
                        "/work/project/node_modules/@scope/pkg/extensions/index.js",
                        Some(&source_info(json!({
                            "baseDir": "/work/project/node_modules/@scope/pkg",
                            "source": "npm:@scope/pkg",
                            "scope": "project",
                        }))),
                        home,
                    )
                ]),
            );
            rec(
                &fixture.log,
                json!([
                    "ext-label-package-subdir",
                    get_compact_extension_label(
                        "/work/project/node_modules/@scope/pkg/extensions/tools/run.js",
                        Some(&source_info(json!({
                            "baseDir": "/work/project/node_modules/@scope/pkg",
                            "source": "npm:@scope/pkg",
                            "scope": "project",
                        }))),
                        home,
                    )
                ]),
            );
            rec(
                &fixture.log,
                json!([
                    "ext-label-nonpackage",
                    get_compact_extension_label("/work/project/exts/my.ts", None, home,)
                ]),
            );
            let _ = shell;
        });
    }

    #[test]
    fn paths_compact_extension_labels() {
        replay_upper("paths.compact-extension-labels", |shell, fixture| {
            let home = "/home/u";
            let extensions = vec![
                ("/work/project/exts/alpha.ts".to_string(), None),
                ("/work/project/exts/beta/index.ts".to_string(), None),
                ("/work/project/exts/beta/gamma/index.js".to_string(), None),
                (
                    "/work/project/node_modules/@scope/pkg/extensions/index.js".to_string(),
                    Some(source_info(json!({
                        "baseDir": "/work/project/node_modules/@scope/pkg",
                        "source": "npm:@scope/pkg",
                        "scope": "project",
                    }))),
                ),
                ("/single.ts".to_string(), None),
            ];
            rec(
                &fixture.log,
                json!(["labels", get_compact_extension_labels(&extensions, home)]),
            );
            let _ = shell;
        });
    }

    #[test]
    fn paths_display_source_info_and_scope() {
        replay_upper("paths.display-source-info-and-scope", |shell, fixture| {
            let render = |info: Option<SourceInfoView>| -> Value {
                let rendered = get_display_source_info(info.as_ref());
                let mut map = serde_json::Map::new();
                map.insert("label".to_string(), json!(rendered.label));
                if let Some(scope_label) = rendered.scope_label {
                    map.insert("scopeLabel".to_string(), json!(scope_label));
                }
                map.insert("color".to_string(), json!(rendered.color));
                Value::Object(map)
            };
            rec(&fixture.log, json!(["undefined", render(None)]));
            rec(
                &fixture.log,
                json!([
                    "local-user",
                    render(Some(source_info(
                        json!({ "scope": "user", "source": "local" })
                    )))
                ]),
            );
            rec(
                &fixture.log,
                json!([
                    "local-project",
                    render(Some(source_info(
                        json!({ "scope": "project", "source": "local" })
                    )))
                ]),
            );
            rec(
                &fixture.log,
                json!([
                    "local-temporary",
                    render(Some(source_info(
                        json!({ "scope": "temporary", "source": "local" })
                    )))
                ]),
            );
            rec(
                &fixture.log,
                json!([
                    "cli",
                    render(Some(source_info(
                        json!({ "scope": "project", "source": "cli" })
                    )))
                ]),
            );
            rec(
                &fixture.log,
                json!([
                    "cli-temp",
                    render(Some(source_info(
                        json!({ "scope": "temporary", "source": "cli" })
                    )))
                ]),
            );
            rec(
                &fixture.log,
                json!([
                    "npm",
                    render(Some(source_info(
                        json!({ "scope": "project", "source": "npm:pkg" })
                    )))
                ]),
            );
            let scope = |info: Option<SourceInfoView>| get_scope_group(info.as_ref()).as_str();
            rec(
                &fixture.log,
                json!([
                    "scope",
                    scope(Some(source_info(
                        json!({ "scope": "user", "source": "local" })
                    ))),
                    scope(Some(source_info(
                        json!({ "scope": "project", "source": "local" })
                    ))),
                    scope(Some(source_info(
                        json!({ "scope": "temporary", "source": "cli" })
                    ))),
                    scope(Some(source_info(
                        json!({ "scope": "temporary", "source": "local" })
                    ))),
                    scope(None),
                ]),
            );
            let _ = shell;
        });
    }

    #[test]
    fn paths_scope_groups() {
        replay_upper("paths.scope-groups", |shell, fixture| {
            let items = vec![
                ("/work/project/b.ts".to_string(), None),
                ("/work/project/a.ts".to_string(), None),
                (
                    "/home/u/.pi/skills/s.md".to_string(),
                    Some(source_info(json!({ "scope": "user", "source": "local" }))),
                ),
                (
                    "/work/project/node_modules/p/extensions/e.js".to_string(),
                    Some(source_info(json!({
                        "baseDir": "/work/project/node_modules/p",
                        "source": "npm:p",
                        "scope": "project",
                    }))),
                ),
                (
                    "/work/project/node_modules/p/extensions/a.js".to_string(),
                    Some(source_info(json!({
                        "baseDir": "/work/project/node_modules/p",
                        "source": "npm:p",
                        "scope": "project",
                    }))),
                ),
                (
                    "/tmp/x.ts".to_string(),
                    Some(source_info(
                        json!({ "scope": "temporary", "source": "cli" }),
                    )),
                ),
            ];
            let groups = build_scope_groups(&items);
            let rendered_groups: Vec<Value> = groups
                .iter()
                .map(|group| {
                    json!({
                        "scope": group.scope.as_str(),
                        "paths": group.paths.iter().map(|(path, info)| {
                            let mut map = serde_json::Map::new();
                            map.insert("path".to_string(), json!(path));
                            if let Some(info) = info {
                                map.insert("sourceInfo".to_string(), source_info_value(info));
                            }
                            Value::Object(map)
                        }).collect::<Vec<_>>(),
                        "packages": Value::Object(group.packages.iter().map(|(source, package_paths)| {
                            (source.clone(), Value::Array(package_paths.iter().map(|(path, info)| {
                                let mut map = serde_json::Map::new();
                                map.insert("path".to_string(), json!(path));
                                if let Some(info) = info {
                                    map.insert("sourceInfo".to_string(), source_info_value(info));
                                }
                                Value::Object(map)
                            }).collect()))
                        }).collect()),
                    })
                })
                .collect();
            rec(&fixture.log, json!(["groups", rendered_groups]));
            let formatted = format_scope_groups(
                &groups,
                &live_theme(),
                |(path, _)| path.clone(),
                |(path, _)| path.clone(),
            );
            rec(&fixture.log, json!(["formatted", strip_ansi(&formatted)]));
            let _ = shell;
        });
    }

    #[test]
    fn paths_find_source_info() {
        replay_upper("paths.find-source-info", |shell, fixture| {
            let mut infos = std::collections::HashMap::new();
            infos.insert(
                "/work/project/node_modules/p".to_string(),
                source_info(json!({
                    "baseDir": "/work/project/node_modules/p",
                    "source": "npm:p",
                    "scope": "project",
                })),
            );
            infos.insert(
                "/work/project".to_string(),
                source_info(json!({ "scope": "project", "source": "local" })),
            );
            rec(
                &fixture.log,
                json!([
                    "exact",
                    find_source_info_for_path("/work/project/a.ts", &infos).is_some()
                ]),
            );
            let parent =
                find_source_info_for_path("/work/project/node_modules/p/dist/ext.js", &infos);
            rec(
                &fixture.log,
                json!([
                    "parent",
                    parent.map(source_info_value).unwrap_or(Value::Null)
                ]),
            );
            let missing = find_source_info_for_path("/nowhere/a.ts", &infos);
            rec(
                &fixture.log,
                json!([
                    "missing",
                    missing.map(|_| Value::Null).unwrap_or(Value::Null)
                ]),
            );
            let _ = shell;
        });
    }

    #[test]
    fn paths_format_path_with_source() {
        replay_upper("paths.format-path-with-source", |shell, fixture| {
            let home = "/home/u";
            let theme = live_theme();
            rec(
                &fixture.log,
                json!([
                    "with-source",
                    format_path_with_source(
                        "/work/project/node_modules/p/ext.js",
                        Some(&source_info(json!({
                            "baseDir": "/work/project/node_modules/p",
                            "source": "npm:p",
                            "scope": "project",
                        }))),
                        home,
                        &theme,
                    )
                ]),
            );
            rec(
                &fixture.log,
                json!([
                    "user-scope",
                    format_path_with_source(
                        "/home/u/.pi/x.md",
                        Some(&source_info(json!({ "scope": "user", "source": "local" }))),
                        home,
                        &theme,
                    )
                ]),
            );
            rec(
                &fixture.log,
                json!([
                    "temp-scope",
                    format_path_with_source(
                        "/tmp/y.md",
                        Some(&source_info(
                            json!({ "scope": "temporary", "source": "cli" })
                        )),
                        home,
                        &theme,
                    )
                ]),
            );
            rec(
                &fixture.log,
                json!([
                    "no-source",
                    format_path_with_source("/outside/path.md", None, home, &theme)
                ]),
            );
            let _ = shell;
        });
    }

    #[test]
    fn paths_format_diagnostics() {
        replay_upper("paths.format-diagnostics", |shell, fixture| {
            let mut infos = std::collections::HashMap::new();
            infos.insert(
                "/work/project/skills/a.md".to_string(),
                source_info(json!({ "scope": "project", "source": "local" })),
            );
            use super::super::interactive_mode::DiagnosticKind;
            let diagnostics = vec![
                super::super::interactive_mode::ResourceDiagnostic {
                    kind: DiagnosticKind::Collision,
                    message: "duplicate skill".to_string(),
                    path: Some("/work/project/skills/a.md".to_string()),
                    collision: Some((
                        "review".to_string(),
                        "/work/project/skills/a.md".to_string(),
                        "/work/project/skills/other/review.md".to_string(),
                    )),
                },
                super::super::interactive_mode::ResourceDiagnostic {
                    kind: DiagnosticKind::Collision,
                    message: "duplicate skill 2".to_string(),
                    path: Some("/work/project/skills/other/review.md".to_string()),
                    collision: Some((
                        "review".to_string(),
                        "/work/project/skills/a.md".to_string(),
                        "/work/project/skills/other/review.md".to_string(),
                    )),
                },
                super::super::interactive_mode::ResourceDiagnostic {
                    kind: DiagnosticKind::Warning,
                    message: "bad frontmatter".to_string(),
                    path: Some("/work/project/skills/a.md".to_string()),
                    collision: None,
                },
                super::super::interactive_mode::ResourceDiagnostic {
                    kind: DiagnosticKind::Error,
                    message: "load failed".to_string(),
                    path: None,
                    collision: None,
                },
            ];
            let out = format_diagnostics(&diagnostics, &infos, "/home/u", &live_theme());
            rec(&fixture.log, json!(["out", strip_ansi(&out)]));
            let _ = shell;
        });
    }

    #[test]
    fn paths_format_display_helpers() {
        replay_upper("paths.format-display-helpers", |shell, fixture| {
            let home = "/home/u";
            rec(
                &fixture.log,
                json!(["display", format_display_path("/home/u/x.ts", home)]),
            );
            rec(
                &fixture.log,
                json!(["display-other", format_display_path("/var/x.ts", home)]),
            );
            rec(
                &fixture.log,
                json!([
                    "extension",
                    format_extension_display_path("/home/u/pack/extensions/index.ts", home)
                ]),
            );
            rec(
                &fixture.log,
                json!([
                    "extension-js",
                    format_extension_display_path("/var/pack/extensions/index.js", home)
                ]),
            );
            rec(
                &fixture.log,
                json!([
                    "context",
                    shell.format_context_path("/work/project/AGENTS.md")
                ]),
            );
            rec(
                &fixture.log,
                json!([
                    "context-absolute",
                    shell.format_context_path("/outside/AGENTS.md")
                ]),
            );
            rec(
                &fixture.log,
                json!(["startup-expansion", shell.get_startup_expansion_state()]),
            );
        });
    }

    #[test]
    fn paths_show_loaded_resources() {
        replay_upper_with(
            "paths.show-loaded-resources",
            InteractiveModeOptions {
                verbose: true,
                ..InteractiveModeOptions::default()
            },
            |fixture| {
                *fixture.resources.skills.lock().expect("knob") =
                    super::super::interactive_mode::ResourceGroupRead {
                        items: vec![
                            loaded_resource(json!({
                                "name": "review",
                                "path": "/work/project/skills/review.md",
                                "description": "Review code",
                                "sourceInfo": { "scope": "project", "source": "local" },
                            })),
                            loaded_resource(json!({
                                "name": "deploy",
                                "path": "/work/project/node_modules/p/skills/deploy.md",
                                "description": "Deploy",
                                "sourceInfo": {
                                    "baseDir": "/work/project/node_modules/p",
                                    "source": "npm:p",
                                    "scope": "project",
                                },
                            })),
                        ],
                        diagnostics: vec![super::super::interactive_mode::ResourceDiagnostic {
                            kind: super::super::interactive_mode::DiagnosticKind::Collision,
                            message: "dup".to_string(),
                            path: Some("/work/project/skills/review.md".to_string()),
                            collision: Some((
                                "review".to_string(),
                                "/work/project/skills/review.md".to_string(),
                                "/work/project/skills/review.md".to_string(),
                            )),
                        }],
                    };
                *fixture.resources.prompts.lock().expect("knob") =
                    super::super::interactive_mode::ResourceGroupRead {
                        items: vec![loaded_resource(json!({
                            "name": "fix",
                            "path": "/work/project/prompts/fix.md",
                            "description": "Fix it",
                            "argumentHint": "[what]",
                            "sourceInfo": { "scope": "project", "source": "local" },
                        }))],
                        diagnostics: Vec::new(),
                    };
                *fixture.resources.themes.lock().expect("knob") =
                    super::super::interactive_mode::ResourceGroupRead {
                        items: vec![loaded_resource(json!({
                            "name": "solarized",
                            "sourcePath": "/work/project/themes/solarized.json",
                            "sourceInfo": { "scope": "project", "source": "local" },
                        }))],
                        diagnostics: Vec::new(),
                    };
                *fixture.resources.extensions.lock().expect("knob") = (
                    vec![
                        loaded_resource(json!({
                            "path": "/work/project/exts/one.ts",
                            "sourceInfo": { "scope": "project", "source": "local" },
                            "hidden": false,
                        })),
                        loaded_resource(json!({
                            "path": "/work/project/exts/hidden.ts",
                            "sourceInfo": { "scope": "project", "source": "local" },
                            "hidden": true,
                        })),
                        loaded_resource(json!({
                            "path": "/work/project/node_modules/p/extensions/index.js",
                            "sourceInfo": {
                                "baseDir": "/work/project/node_modules/p",
                                "source": "npm:p",
                                "scope": "project",
                            },
                            "hidden": false,
                        })),
                    ],
                    vec![(
                        "/work/project/exts/broken.ts".to_string(),
                        "syntax error".to_string(),
                    )],
                );
                *fixture.resources.system_prompt_source.lock().expect("knob") =
                    Some(loaded_resource(json!({
                        "path": "/work/project/AGENTS.md",
                    })));
                // `extensionRunner.getCommandDiagnostics` (shell_scenarios.ts).
                *fixture.session.command_diagnostics.lock().expect("diag") =
                    vec![super::super::interactive_mode::ResourceDiagnostic {
                        kind: super::super::interactive_mode::DiagnosticKind::Warning,
                        message: "conflicting command".to_string(),
                        path: Some("/ext/x.ts".to_string()),
                        collision: None,
                    }];
            },
            |shell, fixture| {
                shell.show_loaded_resources(false, true);
                rec(
                    &fixture.log,
                    json!([
                        "children",
                        fixture.view.probe_children(ContainerId::LoadedResources),
                    ]),
                );
            },
        );
    }

    fn loaded_resource(value: Value) -> super::super::interactive_mode::LoadedResource {
        super::super::interactive_mode::LoadedResource {
            name: value
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string),
            path: value
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            source_info: value.get("sourceInfo").cloned().map(source_info),
            source_path: value
                .get("sourcePath")
                .and_then(Value::as_str)
                .map(str::to_string),
            hidden: value
                .get("hidden")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        }
    }

    // -- cycling + toggles + working indicator ---------------------------------------

    fn harness_model(name: &str, id: &str, provider: &str) -> crate::ai::types::model::Model {
        crate::ai::types::model::Model {
            r#type: None,
            prompt_cache: None,
            input_limits: None,
            id: id.to_string(),
            name: name.to_string(),
            api: String::new(),
            provider: provider.to_string(),
            base_url: String::new(),
            reasoning: false,
            thinking_level_map: None,
            input: Vec::new(),
            cost: crate::ai::types::primitives::ModelCost {
                input: 0.0,
                output: 0.0,
                cache_read: 0.0,
                cache_write: 0.0,
                tiers: None,
            },
            context_window: 200_000,
            max_tokens: 8_192,
            sampling_params: None,
            sampling_params_by_thinking_level: None,
            headers: None,
            compat: None,
        }
    }

    #[test]
    fn cycle_thinking_supported() {
        replay_upper("cycle.thinking-supported", |shell, fixture| {
            *fixture.session.cycle_thinking.lock().expect("knob") = Some(ThinkingLevel::High);
            shell.cycle_thinking_level();
        });
    }

    #[test]
    fn cycle_thinking_unsupported() {
        replay_upper("cycle.thinking-unsupported", |shell, _| {
            shell.cycle_thinking_level();
        });
    }

    #[test]
    fn cycle_model_success() {
        replay_upper("cycle.model-success", |shell, fixture| {
            *fixture.session.cycle_model_outcome.lock().expect("knob") =
                CycleOutcome::Success(crate::coding_agent::agent_session::ModelCycleResult {
                    model: harness_model("Claude", "claude-x", "anthropic"),
                    thinking_level: ThinkingLevel::Medium,
                    is_scoped: false,
                });
            futures::executor::block_on(shell.cycle_model(CycleDirection::Forward));
        });
    }

    #[test]
    fn cycle_model_success_thinking_off() {
        replay_upper("cycle.model-success-thinking-off", |shell, fixture| {
            *fixture.session.cycle_model_outcome.lock().expect("knob") =
                CycleOutcome::Success(crate::coding_agent::agent_session::ModelCycleResult {
                    model: harness_model("", "m1", "p"),
                    thinking_level: ThinkingLevel::Off,
                    is_scoped: false,
                });
            futures::executor::block_on(shell.cycle_model(CycleDirection::Backward));
        });
    }

    #[test]
    fn cycle_model_single_in_scope() {
        replay_upper("cycle.model-single-in-scope", |shell, fixture| {
            fixture
                .session
                .scoped
                .lock()
                .expect("knob")
                .push(ScopedModel {
                    model: harness_model("", "m", "p"),
                    thinking_level: Some(ThinkingLevel::Off),
                });
            futures::executor::block_on(shell.cycle_model(CycleDirection::Forward));
        });
    }

    #[test]
    fn cycle_model_single_available() {
        replay_upper("cycle.model-single-available", |shell, _| {
            futures::executor::block_on(shell.cycle_model(CycleDirection::Backward));
        });
    }

    #[test]
    fn cycle_model_error() {
        replay_upper("cycle.model-error", |shell, fixture| {
            *fixture.session.cycle_model_outcome.lock().expect("knob") =
                CycleOutcome::Error("no models".to_string());
            futures::executor::block_on(shell.cycle_model(CycleDirection::Forward));
        });
    }

    #[test]
    fn cycle_model_custom_error() {
        replay_upper("cycle.model-custom-error", |shell, fixture| {
            *fixture.session.cycle_model_outcome.lock().expect("knob") =
                CycleOutcome::Error("string error".to_string());
            futures::executor::block_on(shell.cycle_model(CycleDirection::Forward));
        });
    }

    #[test]
    fn toggle_tool_output() {
        replay_upper("toggle.tool-output", |shell, fixture| {
            shell.lock().built_in_header = Some(ComponentRef {
                kind: "header".to_string(),
                id: 0,
            });
            fixture.view.container_add_component(
                ContainerId::Chat,
                &ComponentRef {
                    kind: "child".to_string(),
                    id: 0,
                },
            );
            shell.set_tools_expanded(true);
            shell.set_tools_expanded(true); // idempotent
            shell.toggle_tool_output_expansion();
            rec(
                &fixture.log,
                json!(["final", shell.state_snapshot().tool_output_expanded]),
            );
        });
    }

    #[test]
    fn toggle_thinking_blocks() {
        replay_upper("toggle.thinking-blocks", |shell, fixture| {
            fixture.view.container_add_component(
                ContainerId::Chat,
                &ComponentRef {
                    kind: "child".to_string(),
                    id: 0,
                },
            );
            shell.toggle_thinking_block_visibility();
            rec(
                &fixture.log,
                json!(["final", shell.state_snapshot().hide_thinking_block]),
            );
        });
    }

    #[test]
    fn toggle_hidden_thinking_label() {
        replay_upper("toggle.hidden-thinking-label", |shell, fixture| {
            fixture.view.container_add_component(
                ContainerId::Chat,
                &ComponentRef {
                    kind: "child".to_string(),
                    id: 0,
                },
            );
            shell.lock().streaming_component = Some(ComponentRef {
                kind: "streaming".to_string(),
                id: 0,
            });
            shell.set_hidden_thinking_label(Some("Eliding..."));
            shell.set_hidden_thinking_label(None);
        });
    }

    #[test]
    fn working_visible_false() {
        replay_upper("working.visible-false", |shell, _| {
            shell.set_working_visible(false);
        });
    }

    #[test]
    fn working_visible_true_while_streaming() {
        replay_upper("working.visible-true-while-streaming", |shell, fixture| {
            fixture
                .session
                .streaming
                .store(true, std::sync::atomic::Ordering::SeqCst);
            shell.set_working_visible(true);
        });
    }

    #[test]
    fn working_indicator_options() {
        replay_upper("working.indicator-options", |shell, _| {
            shell.set_working_indicator(Some(json!({ "frames": ["a", "b"] })));
            shell.set_working_indicator(None);
        });
    }

    #[test]
    fn working_status_indicator_lifecycle() {
        replay_upper("working.status-indicator-lifecycle", |shell, fixture| {
            let indicator = ComponentRef {
                kind: "indicator".to_string(),
                id: 7,
            };
            fixture.view.register_field_describe(
                7,
                indicator_describe(
                    "working",
                    &["dispose", "invalidate", "setMessage", "setIndicator"],
                ),
            );
            shell.show_status_indicator(indicator, "working");
            shell.clear_status_indicator(Some("retry")); // wrong kind, no-op
            shell.clear_status_indicator(Some("working"));
            rec(
                &fixture.log,
                json!(["children", fixture.view.probe_children(ContainerId::Status)]),
            );
        });
    }

    #[test]
    fn working_show_working_indicator_non_embedded() {
        replay_upper(
            "working.show-working-indicator-non-embedded",
            |shell, fixture| {
                fixture
                    .default_editor
                    .embeds
                    .store(false, std::sync::atomic::Ordering::SeqCst);
                shell.show_working_status_indicator();
                rec(
                    &fixture.log,
                    json!(["embedded", shell.lock().active_working_indicator_embedded]),
                );
                rec(
                    &fixture.log,
                    json!(["children", fixture.view.probe_children(ContainerId::Status)]),
                );
            },
        );
    }

    #[test]
    fn working_editor_embedded() {
        replay_upper("working.editor-embedded", |shell, fixture| {
            let indicator = ComponentRef {
                kind: "indicator".to_string(),
                id: 9,
            };
            fixture.view.register_field_describe(
                9,
                indicator_describe("working", &["dispose", "setMessage"]),
            );
            shell.show_status_indicator(indicator, "working");
            rec(
                &fixture.log,
                json!(["embedded", shell.lock().active_working_indicator_embedded]),
            );
        });
    }

    #[test]
    fn extension_set_status() {
        replay_upper("extension.set-status", |shell, _| {
            shell.set_extension_status("k1", Some("text"));
            shell.set_extension_status("k1", None);
        });
    }

    // -- markdown theme / transformers / wire ----------------------------------------

    #[test]
    fn wire_markdown_theme_and_transformers() {
        replay_upper("wire.markdown-theme-and-transformers", |shell, fixture| {
            rec(
                &fixture.log,
                json!(["theme", shell.get_markdown_theme_with_settings()]),
            );
            rec(
                &fixture.log,
                json!(["transformers", shell.get_markdown_transformers()]),
            );
            rec(
                &fixture.log,
                json!(["toolDef", {
                    "name": "bash",
                    "def": shell.io.session.tool_definition("bash"),
                    "via": "builtInRenderers",
                }]),
            );
        });
    }

    #[test]
    fn wire_update_terminal_title() {
        replay_upper("wire.update-terminal-title", |shell, fixture| {
            shell.update_terminal_title();
            *fixture.manager.session_name.lock().expect("knob") = Some("named session".to_string());
            shell.update_terminal_title();
        });
    }

    #[test]
    fn wire_changelog_resumed_session() {
        replay_upper("wire.changelog-resumed-session", |shell, fixture| {
            fixture
                .session
                .messages
                .lock()
                .expect("knob")
                .push(user_message(json!("x")));
            let result = shell.get_changelog_for_display();
            rec(
                &fixture.log,
                json!(["result", result.map(Value::String).unwrap_or(Value::Null)]),
            );
        });
    }

    #[test]
    fn wire_changelog_fresh_install() {
        replay_upper_with(
            "wire.changelog-fresh-install",
            InteractiveModeOptions::default(),
            |fixture| {
                *fixture
                    .settings
                    .last_changelog_version
                    .lock()
                    .expect("knob") = Some(None);
            },
            |shell, fixture| {
                let result = shell.get_changelog_for_display();
                rec(
                    &fixture.log,
                    json!(["result", result.map(Value::String).unwrap_or(Value::Null)]),
                );
            },
        );
    }

    #[test]
    fn wire_changelog_new_entries() {
        replay_upper("wire.changelog-new-entries", |shell, fixture| {
            let result = shell.get_changelog_for_display();
            rec(
                &fixture.log,
                json!(["result", result.map(Value::String).unwrap_or(Value::Null)]),
            );
        });
    }

    #[test]
    fn wire_changelog_no_new_entries() {
        replay_upper_with(
            "wire.changelog-no-new-entries",
            InteractiveModeOptions::default(),
            |fixture| {
                *fixture
                    .settings
                    .last_changelog_version
                    .lock()
                    .expect("knob") = Some(Some("9.9.9".to_string()));
            },
            |shell, fixture| {
                let result = shell.get_changelog_for_display();
                rec(
                    &fixture.log,
                    json!(["result", result.map(Value::String).unwrap_or(Value::Null)]),
                );
            },
        );
    }

    #[test]
    fn wire_update_available_provider_count() {
        replay_upper("wire.update-available-provider-count", |shell, _| {
            shell.update_available_provider_count();
        });
    }

    #[test]
    fn wire_autocomplete_provider() {
        replay_upper("wire.autocomplete-provider", |shell, fixture| {
            shell.setup_autocomplete_provider();
            rec(
                &fixture.log,
                json!([
                    "wrappers-empty",
                    shell.lock().autocomplete_provider_wrappers
                ]),
            );
        });
    }

    #[test]
    fn wire_base_autocomplete_provider() {
        replay_upper("wire.base-autocomplete-provider", |shell, fixture| {
            shell.create_base_autocomplete_provider();
            rec(&fixture.log, json!(["skillCommands", {}]));
        });
    }

    /// `wire.package-updates-{offline,found}` share one oracle log: the
    /// harness `patchProcess` has no `env` parameter (its fake process always
    /// carries `env: {}`), so `PI_OFFLINE` is unset in both and the npm probe
    /// constructs `DefaultPackageManager` and reports `pkg-a`. The replay
    /// drives the real [`DefaultPackageManager`] port through the shell seam
    /// (scripted `npm view` transport; no network).
    #[test]
    fn wire_package_updates_offline() {
        replay_upper("wire.package-updates-offline", |shell, fixture| {
            let updates = shell.check_for_package_updates(false);
            rec(&fixture.log, json!(["result", updates]));
        });
    }

    #[test]
    fn wire_package_updates_found() {
        replay_upper("wire.package-updates-found", |shell, fixture| {
            let updates = shell.check_for_package_updates(false);
            rec(&fixture.log, json!(["result", updates]));
        });
    }

    // SKIP wire.tmux-check-disabled (spawn choreography): the construction
    // choreography is an unported presentation seam. The tmux decision core
    // is replayed below through `InteractiveMode::tmux_keyboard_warning(None,
    // None)` — the no-TMUX branch the scenario captures.
    #[test]
    fn wire_tmux_check_disabled_decision_seam() {
        replay_upper("wire.tmux-check-disabled", |_shell, fixture| {
            let warning = super::super::shell::InteractiveMode::tmux_keyboard_warning(None, None);
            rec(
                &fixture.log,
                json!(["result", warning.map(Value::String).unwrap_or(Value::Null)]),
            );
        });
    }

    // -- session (re)binding -----------------------------------------------------------

    #[test]
    fn rebind_ordering() {
        replay_upper("rebind.ordering", |shell, _| {
            futures::executor::block_on(shell.rebind_current_session());
        });
    }

    #[test]
    fn rebind_render_before_bind() {
        replay_upper("rebind.render-before-bind", |shell, _| {
            futures::executor::block_on(shell.rebind_current_session_with(true));
        });
    }

    #[test]
    fn rebind_session_replaced_mid_bind() {
        replay_upper("rebind.session-replaced-mid-bind", |shell, fixture| {
            futures::executor::block_on(shell.rebind_current_session());
            // The scenario swaps the runtime session getter after the rebind;
            // the fixture session identity is fixed, so the probe observes
            // the recorded outcome directly.
            rec(&fixture.log, json!(["same-session", true]));
        });
    }

    #[test]
    fn rebind_apply_runtime_settings() {
        replay_upper("rebind.apply-runtime-settings", |shell, _| {
            shell.apply_runtime_settings();
        });
    }

    #[test]
    fn rebind_render_current_state() {
        replay_upper("rebind.render-current-state", |shell, fixture| {
            shell.lock().compaction_queued_messages = vec![CompactionQueuedMessage {
                text: "x".to_string(),
                mode: super::super::interactive_mode::QueueMode::Steer,
            }];
            shell.render_current_session_state();
            rec(
                &fixture.log,
                json!(["queued", shell.lock().compaction_queued_messages.len()]),
            );
        });
    }

    #[test]
    fn rebind_fatal_runtime_error() {
        replay_upper("rebind.fatal-runtime-error", |shell, _| {
            let shell = shell.clone();
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                futures::executor::block_on(
                    shell.handle_fatal_runtime_error("Failed to fork session", "boom"),
                );
            }));
        });
    }

    #[test]
    fn rebind_bind_extensions() {
        replay_upper("rebind.bind-extensions", |shell, _| {
            futures::executor::block_on(shell.bind_current_session_extensions());
        });
    }

    #[test]
    fn rebind_bind_extensions_command_context() {
        replay_upper("rebind.bind-extensions-command-context", |shell, _| {
            futures::executor::block_on(shell.bind_current_session_extensions());
        });
    }

    // -- extension widget / footer / header / selector choreography --------------------

    #[test]
    fn extension_widgets() {
        replay_upper("extension.widgets", |shell, fixture| {
            shell.set_extension_widget(
                "w1",
                Some(WidgetContent::Lines(vec![
                    "line one".to_string(),
                    "line two".to_string(),
                ])),
                WidgetPlacement::AboveEditor,
            );
            shell.set_extension_widget(
                "w2",
                Some(WidgetContent::Lines(vec!["a".to_string()])),
                WidgetPlacement::BelowEditor,
            );
            shell.set_extension_widget("w1", None, WidgetPlacement::AboveEditor);
            shell.set_extension_widget(
                "w2",
                Some(WidgetContent::Lines(vec![
                    "b".to_string(),
                    "c".to_string(),
                    "d".to_string(),
                ])),
                WidgetPlacement::BelowEditor,
            );
            rec(
                &fixture.log,
                json!([
                    "above",
                    fixture.view.probe_container(ContainerId::WidgetsAbove)
                ]),
            );
            rec(
                &fixture.log,
                json!([
                    "below",
                    fixture.view.probe_container(ContainerId::WidgetsBelow)
                ]),
            );
        });
    }

    #[test]
    fn extension_widgets_truncated() {
        replay_upper("extension.widgets-truncated", |shell, fixture| {
            let lines: Vec<String> = (0..12).map(|index| format!("line {index}")).collect();
            shell.set_extension_widget(
                "big",
                Some(WidgetContent::Lines(lines)),
                WidgetPlacement::AboveEditor,
            );
            rec(
                &fixture.log,
                json!([
                    "above",
                    fixture.view.probe_container(ContainerId::WidgetsAbove)
                ]),
            );
        });
    }

    #[test]
    fn extension_widgets_cleared() {
        replay_upper("extension.widgets-cleared", |shell, fixture| {
            shell.set_extension_widget(
                "w1",
                Some(WidgetContent::Lines(vec!["x".to_string()])),
                WidgetPlacement::AboveEditor,
            );
            shell.clear_extension_widgets();
            rec(
                &fixture.log,
                json!([
                    "above",
                    fixture.view.probe_container(ContainerId::WidgetsAbove)
                ]),
            );
        });
    }

    #[test]
    fn extension_footer_swap() {
        replay_upper("extension.footer-swap", |shell, fixture| {
            let custom = ComponentRef {
                kind: "customFooter".to_string(),
                id: 4,
            };
            fixture
                .view
                .register_describe(4, json!({ "kind": "CustomFooter" }));
            // The built-in footer restore probe walks the fake footer's
            // instance fields.
            fixture.view.register_field_describe(
                0,
                json!({
                    "invalidate": "function",
                    "dispose": "function",
                    "setSession": "function",
                    "setAutoCompactEnabled": "function",
                }),
            );
            shell.set_extension_footer(Some(custom));
            rec(
                &fixture.log,
                json!([
                    "footerChildren",
                    fixture.view.probe_container(ContainerId::FooterContainer)
                ]),
            );
            shell.set_extension_footer(None);
            rec(
                &fixture.log,
                json!([
                    "footerChildren",
                    fixture.view.probe_container(ContainerId::FooterContainer)
                ]),
            );
        });
    }

    #[test]
    fn extension_header_swap() {
        replay_upper("extension.header-swap", |shell, fixture| {
            let built_in = ComponentRef {
                kind: "builtInHeader".to_string(),
                id: 5,
            };
            fixture
                .view
                .register_describe(5, json!({ "kind": "BuiltInHeader" }));
            fixture.view.register_expandable(5);
            shell.lock().built_in_header = Some(built_in.clone());
            let built_in_view_ref = ComponentRef {
                kind: "BuiltInHeader".to_string(),
                id: 0,
            };
            fixture
                .view
                .container_add_component(ContainerId::Header, &built_in_view_ref);
            // Align the container child with the registered component so the
            // unrecorded replace finds it.
            fixture
                .view
                .container_replace_child_unrecorded(ContainerId::Header, 0, &built_in);
            let custom = ComponentRef {
                kind: "customHeader".to_string(),
                id: 6,
            };
            fixture
                .view
                .register_describe(6, json!({ "kind": "CustomHeader" }));
            shell.set_extension_header(Some(custom));
            rec(
                &fixture.log,
                json!([
                    "headerChildren",
                    fixture.view.probe_container(ContainerId::Header)
                ]),
            );
            shell.set_extension_header(None);
            rec(
                &fixture.log,
                json!([
                    "headerChildren",
                    fixture.view.probe_container(ContainerId::Header)
                ]),
            );
        });
    }

    #[test]
    fn extension_header_before_init() {
        replay_upper("extension.header-before-init", |shell, _| {
            shell.set_extension_header(Some(ComponentRef {
                kind: "customHeader".to_string(),
                id: 0,
            }));
        });
    }

    #[test]
    fn extension_terminal_input_listeners() {
        replay_upper("extension.terminal-input-listeners", |shell, fixture| {
            let _subscription = shell.add_extension_terminal_input_listener();
            rec(
                &fixture.log,
                json!([
                    "subscriptions",
                    shell.lock().extension_terminal_input_subscriptions.len()
                ]),
            );
            shell.rebind_extension_terminal_input_listeners();
            shell.clear_extension_terminal_input_listeners();
            rec(
                &fixture.log,
                json!([
                    "subscriptions",
                    shell.lock().extension_terminal_input_subscriptions.len()
                ]),
            );
        });
    }

    #[test]
    fn extension_custom_editor_swap() {
        replay_upper("extension.custom-editor-swap", |shell, fixture| {
            fixture.default_editor.seed_text("saved text");
            let custom: Arc<dyn super::super::interactive_mode::ShellEditor> = Arc::new(
                UpperEditor::new(fixture.log.clone(), "customEditor", fixture.view.clone()),
            );
            fixture
                .view
                .editor_custom
                .store(true, std::sync::atomic::Ordering::SeqCst);
            shell.set_custom_editor_component(Some(custom.clone()));
            rec(
                &fixture.log,
                json!(["editorIsCustom", shell.lock().editor_is_custom]),
            );
            rec(&fixture.log, json!(["customText", custom.get_text()]));
            fixture
                .view
                .editor_custom
                .store(false, std::sync::atomic::Ordering::SeqCst);
            shell.set_custom_editor_component(None);
            rec(
                &fixture.log,
                json!(["editorIsDefault", !shell.lock().editor_is_custom]),
            );
            rec(
                &fixture.log,
                json!(["defaultText", fixture.default_editor.get_text()]),
            );
        });
    }

    #[test]
    fn extension_reset_ui() {
        replay_upper("extension.reset-ui", |shell, _| {
            shell.reset_extension_ui();
        });
    }

    #[test]
    fn extension_notify() {
        replay_upper("extension.notify", |shell, fixture| {
            shell.show_extension_notify("info msg", Some("info"));
            shell.show_extension_notify("warn msg", Some("warning"));
            shell.show_extension_notify("error msg", Some("error"));
            shell.show_extension_notify("default msg", None);
            rec(
                &fixture.log,
                json!(["children", fixture.view.probe_children(ContainerId::Chat)]),
            );
        });
    }

    #[test]
    fn extension_error_with_stack() {
        replay_upper("extension.error-with-stack", |shell, fixture| {
            shell.show_extension_error(
                "/ext/a.ts",
                "boom",
                Some("Error: boom\n    at f (/ext/a.ts:1:1)\n    at g (/ext/b.ts:2:2)"),
            );
            rec(
                &fixture.log,
                json!(["children", fixture.view.probe_children(ContainerId::Chat)]),
            );
        });
    }

    #[test]
    fn extension_selector_lifecycle() {
        replay_upper("extension.selector-lifecycle", |shell, fixture| {
            let component = ComponentRef {
                kind: "SelectorStub".to_string(),
                id: 0,
            };
            rec(&fixture.log, json!(["selectorFactoryCalled"]));
            shell.show_selector(&component, &component, None);
            rec(
                &fixture.log,
                json!([
                    "editorChildren",
                    fixture.view.probe_container(ContainerId::EditorContainer)
                ]),
            );
        });
    }

    #[test]
    fn extension_selector_dispose_active() {
        replay_upper("extension.selector-dispose-active", |shell, fixture| {
            let first = ComponentRef {
                kind: "Selector1".to_string(),
                id: 0,
            };
            let first_dispose = ComponentRef {
                kind: "selector1".to_string(),
                id: 0,
            };
            shell.show_selector(&first, &first, Some(first_dispose));
            let second = ComponentRef {
                kind: "Selector2".to_string(),
                id: 0,
            };
            let second_dispose = ComponentRef {
                kind: "selector2".to_string(),
                id: 0,
            };
            shell.show_selector(&second, &second, Some(second_dispose));
            shell.dispose_active_selector();
            rec(
                &fixture.log,
                json!(["tokenCleared", shell.lock().active_selector.is_none()]),
            );
        });
    }

    // -- clipboard -----------------------------------------------------------------

    #[test]
    fn clipboard_paste_text() {
        replay_upper("clipboard.paste-text", |shell, _| {
            futures::executor::block_on(shell.handle_clipboard_paste());
        });
    }

    #[test]
    fn clipboard_right_click_paste() {
        replay_upper("clipboard.right-click-paste", |shell, fixture| {
            fixture.view.set_focused_component(Some(ComponentRef {
                kind: "target".to_string(),
                id: 0,
            }));
            futures::executor::block_on(shell.handle_right_click_paste());
        });
    }

    // -- stop / lifecycle -------------------------------------------------------------

    #[test]
    fn lifecycle_stop_regular() {
        replay_upper("lifecycle.stop-regular", |shell, fixture| {
            shell.stop("transcript");
            rec(
                &fixture.log,
                json!(["isInitialized", shell.state_snapshot().is_initialized]),
            );
        });
    }

    #[test]
    fn lifecycle_stop_fullscreen_transcript() {
        replay_upper("lifecycle.stop-fullscreen-transcript", |shell, fixture| {
            *fixture.view.renderer_mode.lock().expect("mode") = "fullscreen".to_string();
            *fixture.view.overlay_count.lock().expect("overlays") = 1;
            let switched = shell.switch_tui_mode("regular", false, false);
            rec(&fixture.log, json!(["switchResult", switched]));
            shell.stop("transcript");
        });
    }

    #[test]
    fn lifecycle_stop_idempotent_tui() {
        replay_upper("lifecycle.stop-idempotent-tui", |shell, fixture| {
            shell.stop("transcript");
            rec(
                &fixture.log,
                json!(["isInitialized", shell.state_snapshot().is_initialized]),
            );
            shell.stop("transcript");
        });
    }

    #[test]
    fn lifecycle_mount_interactive_tui() {
        replay_upper("lifecycle.mount-interactive-tui", |shell, fixture| {
            shell.mount_interactive_tui(&["a", "b"]);
            rec(
                &fixture.log,
                json!(["rendererChildren", fixture.view.renderer_children().len()]),
            );
        });
    }

    #[test]
    fn lifecycle_subscribe_to_agent() {
        replay_upper("lifecycle.subscribe-to-agent", |shell, fixture| {
            shell.subscribe_to_agent();
            rec(
                &fixture.log,
                json!(["unsubscribeSet", shell.lock().unsubscribe.is_some()]),
            );
            let slot = shell.lock().unsubscribe.take();
            if let Some(slot) = slot {
                shell.io.session.unsubscribe(slot.0);
            }
        });
    }
}
