// The interactive UI glue mirrors upstream callback signatures whose types
// are inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of upstream `coding-agent/src/modes/interactive/interactive-mode.ts`
//! (6648 lines, sha256 `ca84ff33b44af038d71a8b3c6579084fb360bd24358d397604774bcaa21aeb2a`),
//! r18 slice: the interactive session shell — construction/dependency
//! assembly, lifecycle (stop/shutdown/signal choreography), the input ring
//! (key → action dispatch), the AgentSession event → UI bridge, queue and
//! status data sources, resize/mode handling, and the pure label/path helpers.
//!
//! Every decision core was captured as a byte oracle from the REAL upstream
//! file (`tests/fixtures/interactive_r18_oracle/`: the extractor brace-matches the
//! upstream method bodies verbatim and drives 210 scenarios over recording
//! stubs; output `shell_oracle.json`, sha256 `2b7ca1a258a538a7b987a65331af5c500994bc10f57208ced89449e68b7afc1e`).
//! The Rust tests replay the scenarios through the same seams and compare the
//! action log byte-for-byte (`interactive_tests.rs`, `shell_oracle` group).
//!
//! # Seams (presentation, ported by the r19 components slice)
//!
//! The upstream shell mutates TUI components directly. The r18 deterministic
//! core keeps every decision and data shape and hands the *render* to these
//! seams (S-numbers disclosed for the report):
//!
//! - **S1 `ShellView`** — the TUI surface (containers, focus, terminal title/
//!   progress, component construction/updates, overlays, input listeners).
//!   Component identity is a [`ComponentRef`] (kind + id); construction args
//!   are recorded through the view so tests capture the full call surface.
//! - **S2 `ShellEditor`** — the duck-typed `EditorComponent` surface. The
//!   border color travels as a semantic [`EditorBorder`] key
//!   (`color:bashMode` / `color:thinkingMedium`), applied to a string by the
//!   r17-verified [`Theme`] at the same call sites.
//! - **S3 `CommandSink`** — the lower-half command handlers
//!   (`/settings`, `/model`, `/tree`, `/login`, …). The r18 submit ladder and
//!   action ring decide WHICH handler fires with WHICH argument and clear the
//!   editor; the handler bodies are r19.
//! - **S4 `maybeWarnAboutAnthropicSubscriptionAuth`** — warning logic that
//!   needs `modelRuntime.checkAuth/getAuth`; wired as a session-seam hook.
//! - **S5 init()/run() TUI mounting** — `createChatViewport`/
//!   `createInteractiveTui`/`ensureTool` choreography is presentation; the
//!   shell exposes the state transitions and the r19 slice mounts the tree.
//!
//! Styling is the r17-verified theme core ([`theme::Theme`]); the oracle
//! harness ran the same machinery over the byte-identical `dark.json` with
//! fixed chalk-enabled ANSI codes, so recorded strings are byte-exact.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::Value;

use super::bug_report::BugReportOptions;
use super::theme::Theme;
use crate::agent_core::types::{AgentMessage, ThinkingLevel};
use crate::ai::types::primitives::Usage;
use crate::coding_agent::agent_session::{
    AgentSessionError, CycleDirection, ModelCycleResult, ScopedModel,
};
use crate::coding_agent::core::settings_manager::QuietStartup;
use crate::coding_agent::extensions::types::StreamingDelivery;
use crate::coding_agent::session_manager::SessionEntry;

// ===========================================================================
// Shared types
// ===========================================================================

/// Upstream `CompactionQueuedMessage`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionQueuedMessage {
    pub text: String,
    pub mode: QueueMode,
}

/// Upstream `"steer" | "followUp"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueMode {
    Steer,
    FollowUp,
}

impl QueueMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Steer => "steer",
            Self::FollowUp => "followUp",
        }
    }
}

/// Upstream `getAllQueuedMessages()` / `clearAllQueues()` result shape.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueueSnapshot {
    pub steering: Vec<String>,
    pub follow_up: Vec<String>,
}

/// Upstream `CompactionCostNotice`.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactionCostNotice {
    pub kind: CompactionCostKind,
    pub usage: Usage,
}

/// Upstream `"compaction" | "branch_summary"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionCostKind {
    Compaction,
    BranchSummary,
}

/// Options for interactive mode initialization (upstream
/// `InteractiveModeOptions`, presentation-only fields omitted here are
/// mounted by the r19 slice: `terminal`, `initialThemeSetting`).
#[derive(Debug, Clone, Default)]
pub struct InteractiveModeOptions {
    /// Providers that were migrated to auth.json (shows warning).
    pub migrated_providers: Vec<String>,
    /// Warning message if session model couldn't be restored.
    pub model_fallback_message: Option<String>,
    /// Cwd to trust after reload if it gained a .pi directory.
    pub auto_trust_on_reload_cwd: Option<String>,
    /// Initial message to send on startup (can include @file content).
    pub initial_message: Option<String>,
    /// Additional messages to send after the initial message.
    pub initial_messages: Vec<String>,
    /// Force verbose startup (overrides quietStartup setting).
    pub verbose: bool,
    /// TUI layout mode (`"regular" | "fullscreen"` upstream).
    pub tui_mode: Option<String>,
}

/// A component handle crossing the view seam: the kind string mirrors the
/// upstream class name (`AssistantMessageComponent`, `ToolExecutionComponent`,
/// `WorkingStatusIndicator`, …).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentRef {
    pub kind: String,
    pub id: u64,
}

/// Editor border semantic key (upstream assigns a `theme.getThinkingBorderColor(level)`
/// color function; the seam carries the resolved key and the r19 editor
/// resolves it through [`Theme::get_thinking_border_color`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorBorder {
    BashMode,
    Thinking(ThinkingLevel),
}

impl EditorBorder {
    /// The harness tag: `color:bashMode` / `color:thinking<Level>`.
    pub fn tag(&self) -> String {
        match self {
            Self::BashMode => "color:bashMode".to_string(),
            Self::Thinking(level) => format!("color:thinking{}", thinking_level_pascal(level)),
        }
    }
}

/// Upstream `ThinkingLevel` → `thinkingMedium` style PascalCase key.
pub fn thinking_level_pascal(level: &ThinkingLevel) -> String {
    let lower = thinking_level_lower(level);
    let mut chars = lower.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn thinking_level_lower(level: &ThinkingLevel) -> &'static str {
    match level {
        ThinkingLevel::Off => "off",
        ThinkingLevel::Minimal => "minimal",
        ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        ThinkingLevel::High => "high",
        ThinkingLevel::Xhigh => "xhigh",
        ThinkingLevel::Max => "max",
    }
}

// ===========================================================================
// Pure helpers (verbatim upstream logic)
// ===========================================================================

/// Upstream `quoteIfNeeded` (formatResumeCommand argument quoting).
pub fn quote_if_needed(value: &str) -> String {
    let safe = value.bytes().all(|b| {
        b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'/' | b'~' | b':' | b'@')
    });
    if !value.is_empty() && safe {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Upstream `DEAD_TERMINAL_ERROR_CODES` membership (`isDeadTerminalError`).
pub fn is_dead_terminal_error_code(code: Option<&str>) -> bool {
    matches!(code, Some("EIO") | Some("EPIPE") | Some("ENOTCONN"))
}

/// Upstream `ANTHROPIC_SUBSCRIPTION_AUTH_WARNING`.
pub const ANTHROPIC_SUBSCRIPTION_AUTH_WARNING: &str = "Anthropic subscription auth is active. Third-party harness usage draws from extra usage and is billed per token, not your Claude plan limits. Manage extra usage at https://claude.ai/settings/usage. Disable this warning in /settings.";

/// Upstream `isAnthropicSubscriptionAuthKey`.
pub fn is_anthropic_subscription_auth_key(api_key: Option<&str>) -> bool {
    api_key.is_some_and(|key| key.starts_with("sk-ant-oat"))
}

/// Upstream `isUnknownModel`.
pub fn is_unknown_model(provider: Option<&str>, id: Option<&str>, api: Option<&str>) -> bool {
    matches!(
        (provider, id, api),
        (Some("unknown"), Some("unknown"), Some("unknown"))
    )
}

/// Upstream `llamaCppPostLoginGuidance`.
pub fn llama_cpp_post_login_guidance(action_label: &str, loaded_model_count: usize) -> String {
    if loaded_model_count == 0 {
        format!(
            "{action_label}. No llama.cpp models are loaded. Use /llama to load a model, then /model to select it."
        )
    } else {
        format!(
            "{action_label}. Use /model to select a loaded llama.cpp model, or /llama to manage models."
        )
    }
}

/// Upstream `formatTokens` (footer.ts).
pub fn format_tokens(count: f64) -> String {
    if count < 1000.0 {
        return js_number_to_string(count);
    }
    if count < 10_000.0 {
        return format!("{:.1}k", count / 1000.0);
    }
    if count < 1_000_000.0 {
        return format!("{}k", js_round(count / 1000.0));
    }
    if count < 10_000_000.0 {
        return format!("{:.1}M", count / 1_000_000.0);
    }
    format!("{}M", js_round(count / 1_000_000.0))
}

/// JS `Math.round` (half away from zero for the magnitudes used here).
fn js_round(value: f64) -> i64 {
    value.round() as i64
}

/// JS `x.toFixed(2)` for the small non-negative magnitudes that reach the
/// cost notices (exact halves round up, unlike Rust's `{:.2}`).
fn js_fixed_2(value: f64) -> String {
    let scaled = (value * 100.0).round();
    let text = format!("{:.2}", scaled / 100.0);
    text
}

/// JS `Number.prototype.toString` for the small non-negative integers and
/// simple decimals that reach `formatTokens` (`count.toString()` upstream).
fn js_number_to_string(count: f64) -> String {
    if count.fract() == 0.0 && count.abs() < 1e15 {
        format!("{}", count as i64)
    } else {
        format!("{count}")
    }
}

/// Upstream `formatResumeCommand`. `stdout_is_tty` mirrors
/// `process.stdout.isTTY`; `session_file_exists` mirrors the fs.existsSync
/// probe on `sessionManager.getSessionFile()`.
pub fn format_resume_command(
    session_manager: &dyn ShellSessionManager,
    app_name: &str,
    stdout_is_tty: bool,
    session_file_exists: impl Fn(&str) -> bool,
) -> Option<String> {
    if !stdout_is_tty {
        return None;
    }
    if !session_manager.is_persisted() {
        return None;
    }
    let session_file = session_manager.session_file()?;
    if !session_file_exists(&session_file) {
        return None;
    }
    let mut args = vec![app_name.to_string()];
    if !session_manager.uses_default_session_dir() {
        args.push("--session-dir".to_string());
        args.push(quote_if_needed(&session_manager.session_dir()));
    }
    args.push("--session".to_string());
    args.push(session_manager.session_id());
    Some(args.join(" "))
}

/// Upstream `AUTH_TYPE_ORDER` ordering key (`oauth` sorts before `api_key`).
fn auth_type_order(auth_type: &str) -> u8 {
    if auth_type == "oauth" {
        0
    } else {
        1
    }
}

/// Upstream `LoginProviderCompletionOption`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginProviderCompletionOption {
    pub id: String,
    pub name: String,
    pub auth_types: Vec<String>,
}

/// Upstream `AuthSelectorProvider` input shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthSelectorProvider {
    pub id: String,
    pub name: String,
    pub auth_type: String,
}

/// Upstream `getLoginProviderCompletionOptions`: dedupe by id, merge and sort
/// auth types (oauth before api_key), sort by name.
pub fn get_login_provider_completion_options(
    provider_options: &[AuthSelectorProvider],
) -> Vec<LoginProviderCompletionOption> {
    let mut by_id: BTreeMap<String, LoginProviderCompletionOption> = BTreeMap::new();
    for provider in provider_options {
        match by_id.get_mut(&provider.id) {
            Some(existing) => {
                if !existing.auth_types.contains(&provider.auth_type) {
                    existing.auth_types.push(provider.auth_type.clone());
                    existing.auth_types.sort_by_key(|a| auth_type_order(a));
                }
            }
            None => {
                by_id.insert(
                    provider.id.clone(),
                    LoginProviderCompletionOption {
                        id: provider.id.clone(),
                        name: provider.name.clone(),
                        auth_types: vec![provider.auth_type.clone()],
                    },
                );
            }
        }
    }
    let mut options: Vec<LoginProviderCompletionOption> = by_id.into_values().collect();
    options.sort_by(|a, b| a.name.cmp(&b.name));
    options
}

/// Upstream `formatAuthSelectorProviderType` (oauth-selector.ts).
pub fn format_auth_selector_provider_type(auth_type: &str) -> &'static str {
    if auth_type == "oauth" {
        "subscription"
    } else {
        "API key"
    }
}

/// Upstream `getLoginProviderSearchText`.
pub fn get_login_provider_search_text(provider: &LoginProviderCompletionOption) -> String {
    let auth_types = provider
        .auth_types
        .iter()
        .map(|auth_type| {
            format!(
                "{auth_type} {}",
                format_auth_selector_provider_type(auth_type)
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!("{} {} {}", provider.id, provider.name, auth_types)
}

/// Upstream `formatLoginProviderCompletionDescription`.
pub fn format_login_provider_completion_description(
    provider: &LoginProviderCompletionOption,
) -> String {
    let auth_types = provider
        .auth_types
        .iter()
        .map(|auth_type| format_auth_selector_provider_type(auth_type).to_string())
        .collect::<Vec<_>>()
        .join("/");
    if provider.name == provider.id {
        auth_types
    } else {
        format!("{} · {}", provider.name, auth_types)
    }
}

/// Upstream `createFuzzyAutocompleteItems` (over [`crate::tui::fuzzy::fuzzy_filter`]).
pub fn create_fuzzy_autocomplete_items<T>(
    items: Vec<T>,
    prefix: &str,
    get_search_text: impl Fn(&T) -> String,
    to_autocomplete_item: impl Fn(&T) -> AutocompleteItem,
) -> Option<Vec<AutocompleteItem>> {
    // `get_search_text` yields owned strings, so the shared borrow-based
    // `fuzzy_filter` cannot be used directly; mirror its exact semantics
    // (empty query passthrough, token split on whitespace/'/', all-tokens
    // must match, ascending score sort) over the owned texts.
    let query_trim = prefix.trim();
    let filtered: Vec<T> = if query_trim.is_empty() {
        items
    } else {
        let tokens: Vec<String> = query_trim
            .split(|c: char| c.is_whitespace() || c == '/')
            .filter(|token| !token.is_empty())
            .map(str::to_owned)
            .collect();
        let mut results: Vec<(T, f64)> = Vec::new();
        for item in items {
            let text = get_search_text(&item);
            let mut total_score = 0.0;
            let mut all_match = true;
            for token in &tokens {
                let m = crate::tui::fuzzy::fuzzy_match(token, &text);
                if m.matches {
                    total_score += m.score;
                } else {
                    all_match = false;
                    break;
                }
            }
            if all_match {
                results.push((item, total_score));
            }
        }
        results.sort_by(|a, b| a.1.total_cmp(&b.1));
        results.into_iter().map(|(item, _)| item).collect()
    };
    if filtered.is_empty() {
        return None;
    }
    Some(filtered.iter().map(to_autocomplete_item).collect())
}

/// Upstream `AutocompleteItem` (pi-tui) projection used by the shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutocompleteItem {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
}

/// Upstream `SourceInfo` shape the shell reads.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SourceInfoView {
    pub scope: Option<String>,
    pub source: Option<String>,
    pub base_dir: Option<String>,
}

/// Upstream `parseGitUrl` result projection (host/path/ref). The npm/local
/// paths of the oracle exercise this through the shell only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSourceView {
    pub host: String,
    pub path: String,
    pub reference: Option<String>,
}

/// Upstream `getAutocompleteSourceTag`.
pub fn get_autocomplete_source_tag(source_info: Option<&SourceInfoView>) -> Option<String> {
    let source_info = source_info?;
    // Built-in extension commands are untagged, like built-in commands.
    if source_info.source.as_deref() == Some("builtin") {
        return None;
    }
    let scope_prefix = match source_info.scope.as_deref() {
        Some("user") => "u",
        Some("project") => "p",
        _ => "t",
    };
    let source = source_info.source.as_deref().unwrap_or("").trim();
    if source == "auto" || source == "local" || source == "cli" {
        return Some(scope_prefix.to_string());
    }
    if let Some(rest) = source.strip_prefix("npm:") {
        return Some(format!("{scope_prefix}:npm:{rest}"));
    }
    if let Some(git_source) = parse_git_url_view(source) {
        let reference = git_source
            .reference
            .map(|r| format!("@{r}"))
            .unwrap_or_default();
        return Some(format!(
            "{scope_prefix}:git:{}/{}{}",
            git_source.host, git_source.path, reference
        ));
    }
    Some(scope_prefix.to_string())
}

/// Upstream `parseGitUrl` as recorded by the r18 oracle harness: the drive
/// script stubs `parseGitUrl` (drive_shell.ts) with
/// `/^git:\/\/([^/]+)\/(.+)$/` → `{ host, path, ref: undefined }`. The
/// recorded label/tag scenarios are authoritative for the shell's display
/// paths, so the projection mirrors the stub byte for byte (disclosed seam S6).
pub fn parse_git_url_view(source: &str) -> Option<GitSourceView> {
    let rest = source.strip_prefix("git://")?;
    let (host, path) = rest.split_once('/')?;
    // `[^/]+` host and `.+` path are non-empty; JS `.` excludes line
    // terminators.
    if host.is_empty()
        || path.is_empty()
        || path
            .chars()
            .any(|c| matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}'))
    {
        return None;
    }
    Some(GitSourceView {
        host: host.to_string(),
        path: path.to_string(),
        reference: None,
    })
}

/// Upstream `prefixAutocompleteDescription`.
pub fn prefix_autocomplete_description(
    description: Option<&str>,
    source_info: Option<&SourceInfoView>,
) -> Option<String> {
    // Upstream: `if (!sourceTag) return description;` — a missing source tag
    // passes the description through unchanged.
    let Some(source_tag) = get_autocomplete_source_tag(source_info) else {
        return description.map(str::to_string);
    };
    match description {
        Some(description) if !description.is_empty() => {
            Some(format!("[{source_tag}] {description}"))
        }
        _ => Some(format!("[{source_tag}]")),
    }
}

/// Upstream `BUILTIN_SLASH_COMMANDS` projection used by conflict diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionCommandInfo {
    pub name: String,
    pub invocation_name: String,
    pub source_path: Option<String>,
}

/// Upstream `ResourceDiagnostic` shape consumed by `formatDiagnostics`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceDiagnostic {
    pub kind: DiagnosticKind,
    pub message: String,
    pub path: Option<String>,
    /// `(winner_path, loser_path)` for collision diagnostics.
    pub collision: Option<(String, String, String)>,
}

/// Upstream `"error" | "warning" | "collision"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticKind {
    Error,
    Warning,
    Collision,
}

/// Upstream built-in command names the shell checks for conflicts.
pub const BUILTIN_SLASH_COMMAND_NAMES: [&str; 3] = ["model", "thinking", "settings"];

/// Upstream `getBuiltInCommandConflictDiagnostics`.
pub fn get_built_in_command_conflict_diagnostics(
    commands: &[ExtensionCommandInfo],
) -> Vec<ResourceDiagnostic> {
    commands
        .iter()
        .filter(|command| BUILTIN_SLASH_COMMAND_NAMES.contains(&command.name.as_str()))
        .map(|command| ResourceDiagnostic {
            kind: DiagnosticKind::Warning,
            message: if command.invocation_name == command.name {
                format!(
                    "Extension command '/{}' conflicts with built-in interactive command. Skipping in autocomplete.",
                    command.name
                )
            } else {
                format!(
                    "Extension command '/{}' conflicts with built-in interactive command. Available as '/{}'.",
                    command.name, command.invocation_name
                )
            },
            path: command.source_path.clone(),
            collision: None,
        })
        .collect()
}

// ===========================================================================
// Path / label helpers (verbatim upstream logic, posix semantics)
// ===========================================================================

/// Upstream `formatDisplayPath`. `home` is `os.homedir()`.
pub fn format_display_path(path: &str, home: &str) -> String {
    if !home.is_empty() && path.starts_with(home) {
        format!("~{}", &path[home.len()..])
    } else {
        path.to_string()
    }
}

/// Upstream `formatExtensionDisplayPath`.
pub fn format_extension_display_path(path: &str, home: &str) -> String {
    let result = format_display_path(path, home);
    strip_index_tail(&result)
}

fn strip_index_tail(result: &str) -> String {
    let result = match result.strip_suffix("/index.ts") {
        Some(stripped) => stripped.to_string(),
        None => result.to_string(),
    };
    match result.strip_suffix("/index.js") {
        Some(stripped) => stripped.to_string(),
        None => result,
    }
}

/// Upstream `getShortPath`. Posix path semantics (the upstream helpers normalize
/// `\` to `/` first, so posix + this normalization covers both platforms).
pub fn get_short_path(full_path: &str, source_info: Option<&SourceInfoView>, home: &str) -> String {
    let normalized_full_path = full_path.replace('\\', "/");
    let base_dir = source_info.and_then(|s| s.base_dir.as_deref());
    if let (Some(base_dir), true) = (base_dir, is_package_source(source_info)) {
        let normalized_base_dir = base_dir.replace('\\', "/");
        if let Some(npm_root) = npm_root_of(&normalized_base_dir) {
            if let Some(rest) = normalized_full_path.strip_prefix(&format!("{npm_root}/")) {
                // Upstream preserves node_modules-relative topology via
                // path.posix.relative(baseDir, fullPath) when both live under
                // the same node_modules root.
                return posix_relative_from(&normalized_base_dir, &normalized_full_path)
                    .unwrap_or_else(|| rest.to_string());
            }
        }
        let relative = posix_relative_from(base_dir, &normalized_full_path);
        if let Some(relative) = relative {
            if !relative.is_empty() && relative != "." && !relative.starts_with("..") {
                return relative.replace('\\', "/");
            }
        }
    }
    let source = source_info.and_then(|s| s.source.as_deref()).unwrap_or("");
    if let Some(rest) = npm_tail(&normalized_full_path) {
        if source.starts_with("npm:") {
            return rest;
        }
    }
    if source.starts_with("git:") {
        if let Some(rest) = git_tail(&normalized_full_path) {
            return rest;
        }
    }
    format_display_path(full_path, home)
}

fn npm_root_of(base_dir: &str) -> Option<String> {
    // `^(.*\/node_modules)\/(@?[^/]+(?:\/[^/]+)?)$` — the node_modules root
    // plus the package directory (scoped packages carry one extra segment).
    let idx = base_dir.rfind("/node_modules/")?;
    let root = &base_dir[..idx + "/node_modules".len()];
    let package = &base_dir[root.len() + 1..];
    if package.is_empty() {
        return None;
    }
    if package.starts_with('@') {
        if package.matches('/').count() != 1 || package.contains("//") {
            return None;
        }
    } else if package.contains('/') {
        return None;
    }
    Some(root.to_string())
}

fn npm_tail(full_path: &str) -> Option<String> {
    // `node_modules\/(@?[^/]+(?:\/[^/]+)?)\/(.*)`
    let idx = full_path.find("node_modules/")?;
    let rest = &full_path[idx + "node_modules/".len()..];
    let (package, after_package) = rest.split_once('/')?;
    // The optional second segment is greedy for any package name.
    let (package, tail) = match after_package.split_once('/') {
        Some((second, tail)) => (format!("{package}/{second}"), tail),
        None => (package.to_string(), after_package),
    };
    if package.is_empty() || tail.is_empty() {
        return None;
    }
    Some(tail.to_string())
}

fn git_tail(full_path: &str) -> Option<String> {
    // `git\/[^/]+\/[^/]+\/(.*)`
    let idx = full_path.find("git/")?;
    let rest = &full_path[idx + "git/".len()..];
    let mut parts = rest.splitn(3, '/');
    let _host = parts.next()?;
    let _repo = parts.next()?;
    parts.next().map(str::to_string)
}

/// `path.posix.relative(from, to)` for the absolute posix paths used by the
/// label helpers (resolved inputs; no `..` traversal normalization beyond the
/// shared-prefix walk upstream performs for these display paths).
pub fn posix_relative_from(from: &str, to: &str) -> Option<String> {
    let from_parts: Vec<&str> = from.split('/').filter(|p| !p.is_empty()).collect();
    let to_parts: Vec<&str> = to.split('/').filter(|p| !p.is_empty()).collect();
    let mut common = 0;
    while common < from_parts.len()
        && common < to_parts.len()
        && from_parts[common] == to_parts[common]
    {
        common += 1;
    }
    let mut result: Vec<String> = Vec::new();
    for _ in common..from_parts.len() {
        result.push("..".to_string());
    }
    for part in &to_parts[common..] {
        result.push((*part).to_string());
    }
    if result.is_empty() {
        return Some(".".to_string());
    }
    Some(result.join("/"))
}

/// Upstream `getCompactPathLabel`.
pub fn get_compact_path_label(
    resource_path: &str,
    source_info: Option<&SourceInfoView>,
    home: &str,
) -> String {
    let short_path = get_short_path(resource_path, source_info, home);
    let normalized_path = short_path.replace('\\', "/");
    let segments: Vec<&str> = normalized_path
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != "~")
        .collect();
    if let Some(last) = segments.last() {
        return (*last).to_string();
    }
    short_path
}

/// Upstream `getCompactPackageSourceLabel`.
pub fn get_compact_package_source_label(source_info: Option<&SourceInfoView>) -> String {
    let source = source_info.and_then(|s| s.source.as_deref()).unwrap_or("");
    if let Some(rest) = source.strip_prefix("npm:") {
        return if rest.is_empty() {
            source.to_string()
        } else {
            rest.to_string()
        };
    }
    if let Some(git_source) = parse_git_url_view(source) {
        if !git_source.path.is_empty() {
            return git_source.path;
        }
    }
    source.to_string()
}

/// Upstream `isPackageSource`.
pub fn is_package_source(source_info: Option<&SourceInfoView>) -> bool {
    let source = source_info.and_then(|s| s.source.as_deref()).unwrap_or("");
    source.starts_with("npm:") || source.starts_with("git:")
}

/// Upstream `getCompactExtensionLabel`.
pub fn get_compact_extension_label(
    resource_path: &str,
    source_info: Option<&SourceInfoView>,
    home: &str,
) -> String {
    if !is_package_source(source_info) {
        return get_compact_path_label(resource_path, source_info, home);
    }
    let source_label = get_compact_package_source_label(source_info);
    if source_label.is_empty() {
        return get_compact_path_label(resource_path, source_info, home);
    }
    let short_path = get_short_path(resource_path, source_info, home).replace('\\', "/");
    let package_path = short_path
        .strip_prefix("extensions/")
        .map(str::to_string)
        .unwrap_or(short_path);
    // `path.posix.parse(packagePath)` → name (stem) / dir
    let (name, dir) = match package_path.rsplit_once('/') {
        Some((dir, file)) => (file.split('.').next().unwrap_or(file), dir),
        None => (package_path.split('.').next().unwrap_or(&package_path), ""),
    };
    if name == "index" {
        return if dir.is_empty() || dir == "." {
            source_label
        } else {
            format!("{source_label}:{dir}")
        };
    }
    format!("{source_label}:{package_path}")
}

/// Upstream `getCompactDisplayPathSegments`.
pub fn get_compact_display_path_segments(resource_path: &str, home: &str) -> Vec<String> {
    format_display_path(resource_path, home)
        .replace('\\', "/")
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != "~")
        .map(str::to_string)
        .collect()
}

/// Upstream `getCompactNonPackageExtensionLabel`. `all_segments` carries the
/// precomputed display-path segments of every non-package extension, in order.
pub fn get_compact_non_package_extension_label(
    resource_path: &str,
    index: usize,
    all_segments: &[Vec<String>],
    home: &str,
) -> String {
    let segments = match all_segments.get(index) {
        Some(segments) if !segments.is_empty() => segments,
        _ => return get_compact_path_label(resource_path, None, home),
    };
    for segment_count in 1..=segments.len() {
        let candidate = segments[segments.len() - segment_count..].join("/");
        let is_unique = all_segments.iter().enumerate().all(|(item_index, other)| {
            item_index == index
                || other[other.len().saturating_sub(segment_count)..].join("/") != candidate
        });
        if is_unique {
            return candidate;
        }
    }
    segments.join("/")
}

/// Upstream `getCompactExtensionLabels`.
pub fn get_compact_extension_labels(
    extensions: &[(String, Option<SourceInfoView>)],
    home: &str,
) -> Vec<String> {
    // Non-package extensions drop a trailing `index.ts`/`index.js` segment.
    let non_package: Vec<(usize, Vec<String>)> = extensions
        .iter()
        .enumerate()
        .filter_map(|(index, (path, source_info))| {
            if is_package_source(source_info.as_ref()) {
                return None;
            }
            let mut segments = get_compact_display_path_segments(path, home);
            if segments.len() > 1 {
                if let Some(last) = segments.last() {
                    if last == "index.ts" || last == "index.js" {
                        segments.pop();
                    }
                }
            }
            Some((index, segments))
        })
        .collect();

    extensions
        .iter()
        .enumerate()
        .map(|(index, (path, source_info))| {
            if is_package_source(source_info.as_ref()) {
                return get_compact_extension_label(path, source_info.as_ref(), home);
            }
            // `nonPackageIndex` is the position within the non-package list.
            match non_package
                .iter()
                .position(|(original, _)| *original == index)
            {
                Some(non_package_index) => get_compact_non_package_extension_label(
                    path,
                    non_package_index,
                    &non_package
                        .iter()
                        .map(|(_, segments)| segments.clone())
                        .collect::<Vec<_>>(),
                    home,
                ),
                None => get_compact_path_label(path, source_info.as_ref(), home),
            }
        })
        .collect()
}

/// Upstream `getDisplaySourceInfo` result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplaySourceInfo {
    pub label: String,
    pub scope_label: Option<String>,
    pub color: &'static str,
}

/// Upstream `getDisplaySourceInfo`.
pub fn get_display_source_info(source_info: Option<&SourceInfoView>) -> DisplaySourceInfo {
    let source = source_info
        .and_then(|s| s.source.as_deref())
        .unwrap_or("local");
    let scope = source_info
        .and_then(|s| s.scope.as_deref())
        .unwrap_or("project");
    if source == "local" {
        if scope == "user" {
            return DisplaySourceInfo {
                label: "user".into(),
                scope_label: None,
                color: "muted",
            };
        }
        if scope == "project" {
            return DisplaySourceInfo {
                label: "project".into(),
                scope_label: None,
                color: "muted",
            };
        }
        if scope == "temporary" {
            return DisplaySourceInfo {
                label: "path".into(),
                scope_label: Some("temp".into()),
                color: "muted",
            };
        }
        return DisplaySourceInfo {
            label: "path".into(),
            scope_label: None,
            color: "muted",
        };
    }
    if source == "cli" {
        return DisplaySourceInfo {
            label: "path".into(),
            scope_label: if scope == "temporary" {
                Some("temp".into())
            } else {
                None
            },
            color: "muted",
        };
    }
    let scope_label = match scope {
        "user" => Some("user".to_string()),
        "project" => Some("project".to_string()),
        "temporary" => Some("temp".to_string()),
        _ => None,
    };
    DisplaySourceInfo {
        label: source.to_string(),
        scope_label,
        color: "accent",
    }
}

/// Upstream `getScopeGroup` result. `Default` (User) exists only so the
/// `ScopeGrouping` derive can fill the field before explicit assignment in
/// `build_scope_groups`; upstream has no default-constructed group.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ScopeGroup {
    #[default]
    User,
    Project,
    Path,
}

impl ScopeGroup {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::Path => "path",
        }
    }
}

/// Upstream `getScopeGroup`.
pub fn get_scope_group(source_info: Option<&SourceInfoView>) -> ScopeGroup {
    let source = source_info
        .and_then(|s| s.source.as_deref())
        .unwrap_or("local");
    let scope = source_info
        .and_then(|s| s.scope.as_deref())
        .unwrap_or("project");
    if source == "cli" || scope == "temporary" {
        return ScopeGroup::Path;
    }
    match scope {
        "user" => ScopeGroup::User,
        "project" => ScopeGroup::Project,
        _ => ScopeGroup::Path,
    }
}

/// Upstream `buildScopeGroups` group (ordered project → user → path).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScopeGrouping {
    pub scope: ScopeGroup,
    pub paths: Vec<(String, Option<SourceInfoView>)>,
    /// source → items, in insertion order.
    pub packages: Vec<(String, Vec<(String, Option<SourceInfoView>)>)>,
}

/// Upstream `buildScopeGroups`.
pub fn build_scope_groups(items: &[(String, Option<SourceInfoView>)]) -> Vec<ScopeGrouping> {
    let mut project = ScopeGrouping {
        scope: ScopeGroup::Project,
        ..Default::default()
    };
    let mut user = ScopeGrouping {
        scope: ScopeGroup::User,
        ..Default::default()
    };
    let mut path = ScopeGrouping {
        scope: ScopeGroup::Path,
        ..Default::default()
    };
    for (path_str, source_info) in items {
        let group = match get_scope_group(source_info.as_ref()) {
            ScopeGroup::User => &mut user,
            ScopeGroup::Project => &mut project,
            ScopeGroup::Path => &mut path,
        };
        let source = source_info
            .as_ref()
            .and_then(|s| s.source.clone())
            .unwrap_or_else(|| "local".to_string());
        if is_package_source(source_info.as_ref()) {
            match group.packages.iter_mut().find(|(key, _)| *key == source) {
                Some((_, list)) => list.push((path_str.clone(), source_info.clone())),
                None => group
                    .packages
                    .push((source, vec![(path_str.clone(), source_info.clone())])),
            }
        } else {
            group.paths.push((path_str.clone(), source_info.clone()));
        }
    }
    vec![project, user, path]
        .into_iter()
        .filter(|group| !group.paths.is_empty() || !group.packages.is_empty())
        .collect()
}

/// Upstream `formatScopeGroups`. `format_path`/`format_package_path` mirror the
/// caller-provided formatters; colors are applied by the supplied `theme`.
pub fn format_scope_groups(
    groups: &[ScopeGrouping],
    theme: &Theme,
    format_path: impl Fn(&(String, Option<SourceInfoView>)) -> String,
    format_package_path: impl Fn(&(String, Option<SourceInfoView>)) -> String,
) -> String {
    let mut lines: Vec<String> = Vec::new();
    for group in groups {
        lines.push(format!(
            "  {}",
            theme.fg("accent", group.scope.as_str()).unwrap_or_default()
        ));
        let mut sorted_paths = group.paths.clone();
        sorted_paths.sort_by(|a, b| a.0.cmp(&b.0));
        for item in &sorted_paths {
            lines.push(
                theme
                    .fg("dim", &format!("    {}", format_path(item)))
                    .unwrap_or_default(),
            );
        }
        let mut sorted_packages = group.packages.clone();
        sorted_packages.sort_by(|a, b| a.0.cmp(&b.0));
        for (source, items) in &sorted_packages {
            lines.push(format!(
                "    {}",
                theme.fg("mdLink", source).unwrap_or_default()
            ));
            let mut sorted_package_paths = items.clone();
            sorted_package_paths.sort_by(|a, b| a.0.cmp(&b.0));
            for item in &sorted_package_paths {
                lines.push(
                    theme
                        .fg("dim", &format!("      {}", format_package_path(item)))
                        .unwrap_or_default(),
                );
            }
        }
    }
    lines.join("\n")
}

/// Upstream `findSourceInfoForPath`: exact hit, then nearest ancestor.
pub fn find_source_info_for_path<'a>(
    path: &str,
    source_infos: &'a HashMap<String, SourceInfoView>,
) -> Option<&'a SourceInfoView> {
    if let Some(exact) = source_infos.get(path) {
        return Some(exact);
    }
    let mut current = path.to_string();
    while let Some(idx) = current.rfind('/') {
        current.truncate(idx);
        if let Some(parent) = source_infos.get(&current) {
            return Some(parent);
        }
    }
    None
}

/// Upstream `formatPathWithSource` (pure text; colors are applied by callers
/// via the same r17 theme at the identical call sites).
pub fn format_path_with_source(
    path: &str,
    source_info: Option<&SourceInfoView>,
    home: &str,
    theme: &Theme,
) -> String {
    let text = match source_info {
        Some(source_info) => {
            let short_path = get_short_path(path, Some(source_info), home);
            let info = get_display_source_info(Some(source_info));
            let label_text = match info.scope_label {
                Some(scope_label) => format!("{} ({})", info.label, scope_label),
                None => info.label.clone(),
            };
            format!("{label_text} {short_path}")
        }
        None => format_display_path(path, home),
    };
    // Upstream wraps the label in `theme.fg(color, …)` only inside
    // formatDiagnostics; formatPathWithSource itself is plain text.
    let _ = theme;
    text
}

/// Upstream `formatDiagnostics` (semantic text; colors applied via `theme`).
pub fn format_diagnostics(
    diagnostics: &[ResourceDiagnostic],
    source_infos: &HashMap<String, SourceInfoView>,
    home: &str,
    theme: &Theme,
) -> String {
    let mut lines: Vec<String> = Vec::new();

    // Group collision diagnostics by name (insertion order).
    let mut collisions: Vec<(String, Vec<&ResourceDiagnostic>)> = Vec::new();
    let mut other_diagnostics: Vec<&ResourceDiagnostic> = Vec::new();
    for diagnostic in diagnostics {
        if let (DiagnosticKind::Collision, Some((name, _, _))) =
            (diagnostic.kind, diagnostic.collision.as_ref())
        {
            match collisions.iter_mut().find(|(key, _)| key == name) {
                Some((_, list)) => list.push(diagnostic),
                None => collisions.push((name.clone(), vec![diagnostic])),
            }
        } else {
            other_diagnostics.push(diagnostic);
        }
    }

    for (name, collision_list) in &collisions {
        let Some(first) = collision_list.first().and_then(|d| d.collision.as_ref()) else {
            continue;
        };
        lines.push(
            theme
                .fg("warning", &format!("  \"{name}\" collision:"))
                .unwrap_or_default(),
        );
        // Collision tuples are `(name, winnerPath, loserPath)`; upstream
        // formats the winner/loser paths (interactive-mode.ts:1594).
        let winner_info = find_source_info_for_path(&first.1, source_infos);
        lines.push(
            theme
                .fg(
                    "dim",
                    &format!(
                        "    {} {}",
                        theme.fg("success", "✓").unwrap_or_default(),
                        format_path_with_source(&first.1, winner_info, home, theme)
                    ),
                )
                .unwrap_or_default(),
        );
        for diagnostic in collision_list {
            if let Some(collision) = &diagnostic.collision {
                let loser_info = find_source_info_for_path(&collision.2, source_infos);
                lines.push(
                    theme
                        .fg(
                            "dim",
                            &format!(
                                "    {} {} (skipped)",
                                theme.fg("warning", "✗").unwrap_or_default(),
                                format_path_with_source(&collision.2, loser_info, home, theme)
                            ),
                        )
                        .unwrap_or_default(),
                );
            }
        }
    }

    for diagnostic in &other_diagnostics {
        let color = match diagnostic.kind {
            DiagnosticKind::Error => "error",
            _ => "warning",
        };
        match &diagnostic.path {
            Some(path) => {
                let info = find_source_info_for_path(path, source_infos);
                let formatted_path = format_path_with_source(path, info, home, theme);
                lines.push(
                    theme
                        .fg(color, &format!("  {formatted_path}"))
                        .unwrap_or_default(),
                );
                lines.push(
                    theme
                        .fg(color, &format!("    {}", diagnostic.message))
                        .unwrap_or_default(),
                );
            }
            None => {
                lines.push(
                    theme
                        .fg(color, &format!("  {}", diagnostic.message))
                        .unwrap_or_default(),
                );
            }
        }
    }

    lines.join("\n")
}

/// Upstream `countDroppedThinkingBlocks` (static helper): counts
/// `thinking_dropped` transformations in the message diagnostics.
pub fn count_dropped_thinking_blocks(message_diagnostics: Option<&Value>) -> usize {
    let Some(diagnostics) = message_diagnostics.and_then(|d| d.as_array()) else {
        return 0;
    };
    let mut count = 0;
    for diagnostic in diagnostics {
        let Some(diag_type) = diagnostic.get("type").and_then(Value::as_str) else {
            continue;
        };
        if diag_type != "anthropic_input_transformations" {
            continue;
        }
        let Some(transformations) = diagnostic
            .get("details")
            .and_then(|d| d.get("transformations"))
            .and_then(Value::as_array)
        else {
            continue;
        };
        count += transformations
            .iter()
            .filter(|transformation| {
                transformation.get("type").and_then(Value::as_str) == Some("thinking_dropped")
            })
            .count();
    }
    count
}

/// Upstream `addCompactionCostNotice` text construction.
pub fn compaction_cost_notice_text(notice: &CompactionCostNotice, theme: &Theme) -> String {
    let usage = &notice.usage;
    let tokens = usage.input + usage.output + usage.cache_read + usage.cache_write;
    // JS `Number.prototype.toFixed` (half away from zero at the tie).
    let cost = if usage.cost.total >= 0.01 {
        format!(" (~${})", js_fixed_2(usage.cost.total))
    } else {
        String::new()
    };
    let label = match notice.kind {
        CompactionCostKind::Compaction => "Compaction",
        CompactionCostKind::BranchSummary => "Branch summary",
    };
    theme
        .fg(
            "warning",
            &format!(
                "{label}: {} tokens billed{cost}",
                format_tokens(tokens as f64)
            ),
        )
        .unwrap_or_default()
}

/// Upstream `addCacheMissNotice` text construction.
pub fn cache_miss_notice_text(
    missed_tokens: f64,
    missed_cost: f64,
    model_changed: bool,
    idle_ms: f64,
    theme: &Theme,
) -> String {
    if missed_tokens < 20_000.0 && missed_cost < 0.1 {
        return String::new();
    }
    let cost = if missed_cost >= 0.01 {
        format!(" (~${missed_cost:.2})")
    } else {
        String::new()
    };
    let re_billed = format!("{} tokens re-billed{cost}", format_tokens(missed_tokens));
    let label = if model_changed {
        "Cache miss after model switch".to_string()
    } else if idle_ms >= crate::coding_agent::core::cache_stats::CACHE_TTL_MS as f64 {
        format!(
            "Cache miss after {}m idle",
            (idle_ms / 60_000.0).round() as i64
        )
    } else {
        "Cache miss".to_string()
    };
    theme
        .fg("warning", &format!("{label}: {re_billed}"))
        .unwrap_or_default()
}

/// Upstream `formatCrashExtensionHint`: the `/bug`-adjacent hint naming the
/// loaded extensions that appear in a crash's stack frames. `None` matches
/// upstream's `undefined` (no matches or a non-array input).
pub fn format_crash_extension_hint(extension_matches: Option<&[String]>) -> Option<String> {
    let matches: Vec<&str> = extension_matches
        .unwrap_or_default()
        .iter()
        .map(String::as_str)
        .filter(|m| !m.is_empty())
        .collect();
    if matches.is_empty() {
        return None;
    }
    let quoted: Vec<String> = matches.iter().map(|m| format!("`{m}`")).collect();
    let labels = match quoted.len() {
        1 => quoted[0].clone(),
        2 => quoted.join(" and "),
        _ => format!(
            "{}, and {}",
            quoted[..quoted.len() - 1].join(", "),
            quoted[quoted.len() - 1]
        ),
    };
    let noun = if matches.len() == 1 {
        "extension"
    } else {
        "extensions"
    };
    let pronoun = if matches.len() == 1 { "it" } else { "them" };
    let app = crate::coding_agent::cli::APP_NAME;
    Some(format!(
        "A stack frame came from loaded {noun} {labels}, which may be involved. Try disabling {pronoun} with `{app} config`, or run `{app} -ne` to confirm."
    ))
}

/// Upstream `crashReportInstructions`: the "run /bug" line printed after a
/// recorded crash. `has_session_file` mirrors the `session.sessionFile`
/// probe (resume vs fresh start).
pub fn crash_report_instructions(has_session_file: bool) -> String {
    let app = crate::coding_agent::cli::APP_NAME;
    let resume = if has_session_file {
        format!("run `{app} -r` to resume the session, then")
    } else {
        format!("start {app} and")
    };
    format!(
        "To report this crash: {resume} run /bug. The crash details are attached automatically."
    )
}

/// The `\b(?:abort(?:ed)?|cancel(?:l?ed)?)\b` probe of
/// `maybeSuggestBugReport` (case-insensitive, whole word).
pub(crate) fn abort_or_cancel_word(message: &str) -> bool {
    // Inline scan: word-boundary match over the case-insensitive
    // alternation without pulling a regex engine into the shell core.
    let lower = message.to_lowercase();
    let bytes = lower.as_bytes();
    let is_word = |i: usize| {
        bytes
            .get(i)
            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
    };
    // The alternation is ASCII, so only ASCII positions can start a match
    // (JS `\b`/`\w` are ASCII too); skip byte offsets inside multibyte chars.
    for start in 0..bytes.len() {
        if !bytes[start].is_ascii() {
            continue;
        }
        for word in ["aborted", "abort", "cancelled", "canceled", "cancel"] {
            if lower[start..].starts_with(word) {
                let end = start + word.len();
                // \b on both sides: neither neighbor may be a word char
                // (JS `\b` is exactly this at string edges).
                if (start == 0 || !is_word(start - 1)) && !is_word(end) {
                    return true;
                }
            }
        }
    }
    false
}

/// Upstream `maybeSuggestBugReport`'s decision core: an assistant message
/// that ended in a non-retryable, non-cancellation error suggests `/bug`
/// (once per session — the latch lives in the shell state).
pub fn should_suggest_bug_report(
    stop_reason: Option<&str>,
    error_message: Option<&str>,
    retryable: bool,
) -> bool {
    if stop_reason != Some("error") || retryable {
        return false;
    }
    if error_message.is_some_and(abort_or_cancel_word) {
        return false;
    }
    true
}

/// Upstream `suggestBugReport` hint line.
pub fn bug_report_hint_text() -> String {
    let app = crate::coding_agent::cli::APP_NAME;
    format!("If this looks like a {app} bug, /bug sends a report to the developers.")
}

/// Upstream startup crash warning (the `takeUnnotifiedCrash` notice).
pub fn crash_notice_text(when: &str, message: &str) -> String {
    let app = crate::coding_agent::cli::APP_NAME;
    format!(
        "{app} crashed on {when} ({message}). Run /bug to report it; the crash details are attached automatically."
    )
}

// ===========================================================================
// Shell seams (see the module header for the S-numbers)
// ===========================================================================

/// Upstream `Date.now()`.
pub trait ShellClock: Send + Sync {
    fn now_ms(&self) -> i64;
}

/// `Date.now` over the wall clock.
pub struct SystemClock;
impl ShellClock for SystemClock {
    fn now_ms(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}

/// The subset of `SettingsManager` the shell reads (upstream
/// `this.settingsManager.*` call surface). Implemented for the real
/// [`crate::coding_agent::core::settings_manager::SettingsManager`] below.
pub trait ShellSettings: Send + Sync {
    /// Upstream `getQuietStartup()` (v1.0.0): `boolean | "header"`.
    fn quiet_startup(&self) -> QuietStartup;
    fn show_terminal_progress(&self) -> bool;
    fn double_escape_action(&self) -> String;
    fn hide_thinking_block(&self) -> bool;
    fn set_hide_thinking_block(&self, value: bool);
    fn show_cache_miss_notices(&self) -> bool;
    fn collapse_changelog(&self) -> bool;
    fn output_pad(&self) -> i64;
    fn editor_padding_x(&self) -> i64;
    fn autocomplete_max_visible(&self) -> i64;
    fn clear_on_shrink(&self) -> bool;
    fn show_hardware_cursor(&self) -> bool;
    fn fullscreen_scrollbar(&self) -> bool;
    fn fullscreen_copy_on_select(&self) -> bool;
    fn fullscreen_exit_output(&self) -> String;
    fn code_block_indent(&self) -> String;
    fn enable_skill_commands(&self) -> bool;
    fn last_changelog_version(&self) -> Option<String>;
    fn set_last_changelog_version(&self, version: &str);
    fn show_images(&self) -> bool;
    fn image_width_cells(&self) -> i64;
    fn project_trusted(&self) -> bool;
    fn http_idle_timeout_ms(&self) -> Option<u64>;
    // -- lower-half reads ----------------------------------------------------
    fn default_provider(&self) -> Option<String>;
    fn default_model(&self) -> Option<String>;
    fn default_thinking_level(&self) -> Option<ThinkingLevel>;
    fn enabled_models(&self) -> Option<Vec<String>>;
    fn branch_summary_skip_prompt(&self) -> bool;
    fn external_editor_command(&self) -> String;
    /// Upstream `getWarnings()` (the `anthropicExtraUsage` flag decides the
    /// Anthropic subscription warning).
    fn warnings_anthropic_extra_usage(&self) -> bool;
    fn image_auto_resize(&self) -> bool;
    fn block_images(&self) -> bool;
    fn transport(&self) -> String;
    fn default_project_trust(&self) -> String;
    fn tree_filter_mode(&self) -> String;
    fn enable_install_telemetry(&self) -> bool;
    fn mermaid_rendering_mode(&self) -> String;
    fn theme(&self) -> String;
    /// Setter pump: records/routes `settings.<name>` writes (S1a). `name` is
    /// the upstream setter suffix (`setTheme` → `"setTheme"`).
    fn set(&self, name: &str, value: Value);
}

/// The subset of `SessionManager` the shell reads.
pub trait ShellSessionManager: Send + Sync {
    fn cwd(&self) -> String;
    fn is_persisted(&self) -> bool;
    fn session_file(&self) -> Option<String>;
    fn session_id(&self) -> String;
    fn session_dir(&self) -> String;
    fn uses_default_session_dir(&self) -> bool;
    fn session_name(&self) -> Option<String>;
    fn build_context_entries(&self) -> Vec<SessionEntry>;
    fn entries(&self) -> Vec<SessionEntry>;
    fn branch(&self) -> Vec<SessionEntry>;
    /// `getTree()` — the tree selector input (`(id, type)` rows).
    fn tree(&self) -> Vec<(String, String)>;
    fn leaf_id(&self) -> Option<String>;
    fn append_label_change(&self, entry_id: &str, label: &str);
    fn append_session_info(&self, name: &str);
}

/// The extension-runner surface the shell consumes.
pub trait ShellExtensionSurface: Send + Sync {
    /// Upstream `extensionRunner.getCommand(name)` — invoked WITHOUT the
    /// leading slash by `isExtensionCommand`.
    fn has_command(&self, name: &str) -> bool;
    fn registered_commands(&self) -> Vec<ExtensionCommandInfo>;
    fn command_diagnostics(&self) -> Vec<ResourceDiagnostic>;
    fn shortcut_diagnostics(&self) -> Vec<ResourceDiagnostic>;
    fn markdown_transformers(&self) -> Vec<String>;
    fn has_entry_renderer(&self, custom_type: &str) -> bool;
    fn has_message_renderer(&self, custom_type: &str) -> bool;
    /// Setter pump for runner-side calls (S1a): `getShortcuts` config probes,
    /// `getModelRegistry`, `emitUserBash` transport.
    fn emit(&self, tuple: Value);
}

/// The `AgentSession` surface the shell consumes. Implemented for
/// `Arc<AgentSession>` below; tests inject recorders.
pub trait ShellSession: Send + Sync {
    fn is_streaming(&self) -> bool;
    fn is_compacting(&self) -> bool;
    fn is_bash_running(&self) -> bool;
    fn is_idle(&self) -> bool;
    fn thinking_level(&self) -> ThinkingLevel;
    fn retry_attempt(&self) -> u32;
    fn pending_message_count(&self) -> usize;
    fn scoped_models(&self) -> Vec<ScopedModel>;
    fn steering_messages(&self) -> Vec<String>;
    fn follow_up_messages(&self) -> Vec<String>;
    fn clear_queue(&self) -> (Vec<String>, Vec<String>);
    fn prompt(
        &self,
        text: String,
        streaming_behavior: Option<StreamingDelivery>,
    ) -> BoxFuture<'_, Result<(), AgentSessionError>>;
    fn steer(&self, text: String) -> BoxFuture<'_, Result<(), AgentSessionError>>;
    fn follow_up(&self, text: String) -> BoxFuture<'_, Result<(), AgentSessionError>>;
    fn abort(&self);
    fn abort_bash(&self);
    fn abort_compaction(&self);
    fn abort_retry(&self);
    fn cycle_thinking_level(&self) -> Option<ThinkingLevel>;
    fn cycle_model(
        &self,
        direction: CycleDirection,
    ) -> BoxFuture<'_, Result<Option<ModelCycleResult>, AgentSessionError>>;
    fn extensions(&self) -> Arc<dyn ShellExtensionSurface>;
    /// Upstream `void this.maybeWarnAboutAnthropicSubscriptionAuth(model)` —
    /// the warning decision core (S4).
    fn maybe_warn_anthropic_subscription_auth(&self, provider: Option<&str>);
    // -- lower half -----------------------------------------------------------
    /// Subscribe to session events; the returned slot identifies the
    /// subscription for `stop()`'s unsubscribe.
    fn subscribe(&self) -> u64;
    fn unsubscribe(&self, slot: u64);
    fn model(&self) -> Option<ModelRef>;
    fn model_runtime(&self) -> Arc<dyn ShellModelRuntime>;
    fn resources(&self) -> Arc<dyn ShellResources>;
    fn shortcuts(&self) -> Arc<dyn ShellShortcutSurface>;
    fn available_thinking_levels(&self) -> Vec<ThinkingLevel>;
    fn set_model(&self, model: &ModelRef, persist: bool) -> BoxFuture<'_, Result<(), String>>;
    fn set_thinking_level(&self, level: ThinkingLevel, persist: bool) -> Result<(), String>;
    fn set_scoped_models(&self, models: &[ModelRef]);
    fn auto_compaction_enabled(&self) -> bool;
    fn set_auto_compaction_enabled(&self, enabled: bool);
    fn steering_mode(&self) -> Value;
    fn follow_up_mode(&self) -> Value;
    fn set_steering_mode(&self, mode: Value);
    fn set_follow_up_mode(&self, mode: Value);
    fn user_messages_for_forking(&self) -> Vec<ForkableUserMessage>;
    fn session_stats(&self) -> SessionStats;
    fn last_assistant_text(&self) -> Option<String>;
    fn set_session_name(&self, name: &str);
    /// Setter pump for the remaining session writes (S1a):
    /// `["session.setX", …]`.
    fn emit(&self, tuple: Value);
    fn navigate_tree(
        &self,
        entry_id: &str,
        summarize: bool,
        custom_instructions: Option<&str>,
    ) -> BoxFuture<'_, Result<NavigateOutcome, String>>;
    fn abort_branch_summary(&self);
    fn compact(&self, custom_instructions: Option<&str>) -> BoxFuture<'_, Result<(), String>>;
    /// `executeBash(command, onChunk, options)` — chunk callbacks travel as
    /// [`BashChunkSink`].
    fn execute_bash(
        &self,
        command: &str,
        exclude_from_context: bool,
        chunk_sink: &dyn Fn(&str),
    ) -> BoxFuture<'_, Result<BashOutcome, String>>;
    fn record_bash_result(&self, command: &str, result: &BashOutcome, exclude_from_context: bool);
    fn reload(&self, before_session_start: Option<&dyn Fn()>) -> BoxFuture<'_, Result<(), String>>;
    fn export_to_jsonl(&self, path: &str) -> Result<String, String>;
    /// `buildBundle` + the delivery of `/bug` (upstream assembles
    /// `collectBugReportMetadata`/`collectBugReportDiagnostics`/
    /// `serializeSessionBranch` and either uploads or writes the zip from the
    /// interactive layer). The session surface carries those reads; the
    /// projection reports the outcome the flow's statuses need.
    fn build_bug_report_bundle(
        &self,
        options: BugReportOptions,
        summary: Option<String>,
    ) -> BoxFuture<'_, Result<BugReportOutcome, String>>;
    /// `session.summarizeForBugReport({ hint })` (core/bug-report summary).
    fn summarize_for_bug_report(
        &self,
        hint: Option<&str>,
    ) -> BoxFuture<'_, Result<String, String>> {
        let _ = hint;
        Box::pin(async { Err("summarizeForBugReport unavailable".to_string()) })
    }
    fn export_to_html(
        &self,
        path: Option<&str>,
        theme_name: &str,
    ) -> BoxFuture<'_, Result<String, String>>;
    fn tool_definition(&self, name: &str) -> Value;
    fn context_usage(&self) -> Option<Value>;
    fn system_prompt(&self) -> String;
    /// Raw `session.messages` for the debug log / changelog skip.
    fn messages(&self) -> Vec<AgentMessage>;
    fn wait_for_idle(&self) -> BoxFuture<'_, ()>;
    /// `bindExtensions({ uiContext, mode: "tui", … })` — the context travels
    /// as its describe projection (S8).
    fn bind_extensions(&self, context: Value) -> BoxFuture<'_, ()>;
    /// `detectCacheMiss(entries, message, runtime)` (cache-stats core).
    fn detect_cache_miss(&self, message: &AgentMessage) -> Option<CacheMiss>;
    /// `collectCacheMisses(entries, runtime)` for the replay rendering, as
    /// `(assistant-message json, miss)` pairs in entry order.
    fn collect_cache_misses(&self) -> Vec<(Value, CacheMiss)>;
}

/// Upstream `CacheMiss` (cache-stats core projection).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CacheMiss {
    pub missed_tokens: f64,
    pub missed_cost: f64,
    pub model_changed: bool,
    pub idle_ms: f64,
}

/// The duck-typed `EditorComponent` surface (S2).
pub trait ShellEditor: Send + Sync {
    fn get_text(&self) -> String;
    fn get_expanded_text(&self) -> String;
    fn set_text(&self, text: &str);
    fn add_to_history(&self, text: &str);
    fn insert_text_at_cursor(&self, text: &str);
    fn set_border_color(&self, border: EditorBorder);
    /// `editor.borderColor` read-back (custom-editor copying).
    fn border_color(&self) -> Option<String>;
    /// `editor.getCursor?.()` — the `(line, col)` of the caret; `None` when
    /// the editor has no cursor probe (upstream's optional call).
    fn get_cursor(&self) -> Option<(usize, usize)> {
        None
    }
    /// `editor.handleInput(data)` (paste-to-editor bridge).
    fn handle_input(&self, data: &str);
    fn set_working_status_indicator(&self, indicator: Option<ComponentRef>);
    fn set_autocomplete_provider(&self);
    fn set_padding_x(&self, px: i64);
    fn set_autocomplete_max_visible(&self, n: i64);
    fn get_padding_x(&self) -> i64;
    fn get_autocomplete_max_visible(&self) -> i64;
    /// Upstream `defaultEditor.onAction(action, handler)` registrations.
    fn on_action(&self, action: &'static str);
    fn set_on_escape(&self);
    fn set_on_ctrl_d(&self);
    fn set_on_submit(&self);
    fn set_on_change(&self);
    fn set_on_paste_image(&self);
    fn set_on_extension_shortcut(&self, enabled: bool);
    /// Whether this editor embeds a working-status indicator
    /// (`isWorkingStatusEditor`).
    fn embeds_working_status(&self) -> bool;
}

/// Containers the shell mutates (upstream field names).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerId {
    LoadedResources,
    Chat,
    PendingMessages,
    Status,
    WidgetsAbove,
    WidgetsBelow,
    Header,
    EditorContainer,
    FooterContainer,
    Document,
}

impl ContainerId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LoadedResources => "loadedResources",
            Self::Chat => "chat",
            Self::PendingMessages => "pendingMessages",
            Self::Status => "status",
            Self::WidgetsAbove => "widgetsAbove",
            Self::WidgetsBelow => "widgetsBelow",
            Self::Header => "header",
            Self::EditorContainer => "editorContainer",
            Self::FooterContainer => "footer",
            Self::Document => "document",
        }
    }
}

/// Focus target (`this.editor` or a named component).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FocusTarget {
    Editor,
    Component(ComponentRef),
    None,
}

/// Component construction requests (S1). `args` mirror the upstream
/// constructor arguments in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentKind {
    AssistantMessage,
    ToolExecution,
    UserMessage,
    BashExecution,
    CompactionSummaryMessage,
    BranchSummaryMessage,
    SkillInvocationMessage,
    CustomMessage,
    CustomEntry,
    WorkingStatusIndicator,
    CompactionStatusIndicator,
    RetryStatusIndicator,
    BranchSummaryStatusIndicator,
    ExtensionSelector,
    ExtensionInput,
    ExtensionEditor,
    SettingsSelector,
    ThinkingSelector,
    ModelSelector,
    ScopedModelsSelector,
    UserMessageSelector,
    TreeSelector,
    SessionSelector,
    TrustSelector,
    LoginDialog,
    OAuthSelector,
    ExtensionSelectorDialog,
    ExtensionInputDialog,
    ExtensionEditorDialog,
    Armin,
    Earendil,
}

impl ComponentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AssistantMessage => "AssistantMessageComponent",
            Self::ToolExecution => "ToolExecutionComponent",
            Self::UserMessage => "UserMessageComponent",
            Self::BashExecution => "BashExecutionComponent",
            Self::CompactionSummaryMessage => "CompactionSummaryMessageComponent",
            Self::BranchSummaryMessage => "BranchSummaryMessageComponent",
            Self::SkillInvocationMessage => "SkillInvocationMessageComponent",
            Self::CustomMessage => "CustomMessageComponent",
            Self::CustomEntry => "CustomEntryComponent",
            Self::WorkingStatusIndicator => "WorkingStatusIndicator",
            Self::CompactionStatusIndicator => "CompactionStatusIndicator",
            Self::RetryStatusIndicator => "RetryStatusIndicator",
            Self::BranchSummaryStatusIndicator => "BranchSummaryStatusIndicator",
            Self::ExtensionSelector => "ExtensionSelectorComponent",
            Self::ExtensionInput => "ExtensionInputComponent",
            Self::ExtensionEditor => "ExtensionEditorComponent",
            Self::SettingsSelector => "SettingsSelectorComponent",
            Self::ThinkingSelector => "ThinkingSelectorComponent",
            Self::ModelSelector => "ModelSelectorComponent",
            Self::ScopedModelsSelector => "ScopedModelsSelectorComponent",
            Self::UserMessageSelector => "UserMessageSelectorComponent",
            Self::TreeSelector => "TreeSelectorComponent",
            Self::SessionSelector => "SessionSelectorComponent",
            Self::TrustSelector => "TrustSelectorComponent",
            Self::LoginDialog => "LoginDialogComponent",
            Self::OAuthSelector => "OAuthSelectorComponent",
            Self::ExtensionSelectorDialog => "ExtensionSelectorComponent",
            Self::ExtensionInputDialog => "ExtensionInputComponent",
            Self::ExtensionEditorDialog => "ExtensionEditorComponent",
            Self::Armin => "ArminComponent",
            Self::Earendil => "EarendilAnnouncementComponent",
        }
    }
}

/// The TUI surface the shell drives (S1). Implementations mount the real
/// component tree in r19; tests record every call.
pub trait ShellView: Send + Sync {
    // renderer / terminal
    fn request_render(&self, force: Option<bool>);
    fn invalidate(&self);
    fn render_now(&self);
    fn start(&self);
    fn stop(&self, preserve_screen: bool);
    fn set_clear_on_shrink(&self, enabled: bool);
    fn set_show_hardware_cursor(&self, enabled: bool);
    fn terminal_set_progress(&self, enabled: bool);
    fn terminal_set_title(&self, title: &str);
    fn drain_input(&self, ms: u64);
    fn set_focus(&self, target: FocusTarget);
    // overlays / input listeners
    fn show_overlay(&self, component: &ComponentRef, options: Option<Value>);
    fn hide_overlay(&self);
    fn add_input_listener(&self) -> u64;
    fn remove_input_listener(&self, id: u64);
    // containers
    fn container_version(&self, container: ContainerId) -> u64;
    fn container_clear(&self, container: ContainerId);
    fn container_add_spacer(&self, container: ContainerId) -> u64;
    /// Adds a (possibly truncated) text row with the pre-styled string.
    fn container_add_text(
        &self,
        container: ContainerId,
        text: &str,
        pad_x: i64,
        pad_y: i64,
        truncated: bool,
    ) -> u64;
    /// Adds an upstream `ExpandableText` row: the component carries both body
    /// getters and initially renders the body picked by
    /// `getStartupExpansionState()`.
    fn container_add_expandable_text(
        &self,
        container: ContainerId,
        collapsed: &str,
        expanded: &str,
        initially_expanded: bool,
        pad_x: i64,
        pad_y: i64,
    ) -> u64;
    fn container_set_text(&self, container: ContainerId, id: u64, text: &str);
    fn container_add_component(&self, container: ContainerId, component: &ComponentRef);
    fn container_remove_component(&self, container: ContainerId, component: &ComponentRef);
    fn container_replace_child(
        &self,
        container: ContainerId,
        index: usize,
        component: &ComponentRef,
    );
    fn container_replace_child_unrecorded(
        &self,
        container: ContainerId,
        index: usize,
        component: &ComponentRef,
    );
    fn container_children_len(&self, container: ContainerId) -> usize;
    /// Component construction with the upstream constructor arguments.
    fn new_component(&self, kind: ComponentKind, args: Value) -> ComponentRef;
    /// Component method calls (`updateContent`, `updateResult`, ...).
    fn update_component(&self, component: &ComponentRef, op: &str, args: Value);
    /// Raw collaborator-event tuple for the surfaces that carry no decision
    /// (S1a): `["footer.invalidate"]`, `["terminal.setTitle", t]`,
    /// `["settings.setTheme", v]`, `["themeController.preview", n]`, … The
    /// recording view appends the tuple verbatim; the production view routes
    /// it to the matching collaborator.
    fn emit(&self, tuple: Value);
    // -- lower-half reads ----------------------------------------------------
    fn get_clear_on_shrink(&self) -> bool;
    fn idle_status_component(&self) -> ComponentRef;
    fn has_overlay_entries(&self) -> bool;
    /// The renderer/ui mode string (`"regular" | "fullscreen"`).
    fn renderer_mode(&self) -> String;
    fn renderer_children(&self) -> Vec<ComponentRef>;
    fn renderer_focused_component(&self) -> Option<ComponentRef>;
    fn renderer_terminal_id(&self) -> u64;
    fn renderer_show_hardware_cursor(&self) -> bool;
    fn renderer_capture_render_state(&self);
    fn renderer_create(&self, mode: &str, terminal: u64) -> u64;
    fn renderer_become(&self, id: u64);
    fn renderer_stop_preserving_screen(&self);
    fn renderer_set_focus_none(&self);
    fn renderer_clear(&self);
    fn renderer_set_layout_root_none(&self);
    fn renderer_invalidate(&self);
    fn renderer_start(&self);
    fn renderer_add_child(&self, container: ContainerId);
    /// `headerContainer.children.unshift(component)`.
    fn header_unshift(&self, component: &ComponentRef);
    /// The renderer-level `hideOverlay` of `stopInteractiveTui` (distinct
    /// from the shell `ui.hideOverlay`).
    fn renderer_hide_overlay(&self);
    /// The renderer-level `renderNow` of `stopInteractiveTui`.
    fn renderer_render_now(&self);
    /// Mounts a shell-owned container by name into the live renderer.
    fn renderer_add_child_by_name(&self, name: &str);
    /// The focused-component probe of `handleRightClickPaste`/`handleCopyCommand`.
    fn get_copy_on_select(&self) -> bool;
    fn has_active_selection(&self) -> bool;
    /// Terminal metrics + rendered lines for `handleDebugCommand`.
    fn debug_render(&self) -> (usize, usize, Vec<String>);
    /// `(chat children, status children)` snapshot helpers used by the
    /// `setToolsExpanded`/thinking sweeps.
    fn container_components(&self, container: ContainerId) -> Vec<ComponentRef>;
    fn container_insert_at(&self, container: ContainerId, index: usize, component: &ComponentRef);
    fn container_add_border(&self, container: ContainerId, color_tag: Option<&str>) -> u64;
    fn container_add_markdown(
        &self,
        container: ContainerId,
        text: &str,
        pad_x: i64,
        pad_y: i64,
        theme: &Value,
    ) -> u64;
    fn container_remove_at(&self, container: ContainerId, component: &ComponentRef);
    /// Stable identity of the bound session (`rebindCurrentSession` guard).
    fn session_identity(&self) -> u64;
    /// Resolves a pending `getUserInput` waiter.
    fn user_input_resolved(&self, slot: u64, text: &str);
    /// The focused component's `handleInput` (right-click bracketed paste);
    /// `false` when the component has no input handler.
    fn component_handle_input(&self, component: &ComponentRef, data: &str) -> bool;
}

/// The `AgentSessionRuntime` surface (constructor wiring, dispose, session
/// replacement commands).
pub trait ShellHost: Send + Sync {
    fn set_before_session_invalidate(&self, hook: Option<Box<dyn Fn() + Send + Sync>>);
    fn set_rebind_session(
        &self,
        hook: Option<Box<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>>,
    );
    fn dispose(&self) -> BoxFuture<'_, ()>;
    fn new_session(&self, options: Value) -> BoxFuture<'_, Result<NewSessionOutcome, String>>;
    fn fork(&self, entry_id: &str, options: Value) -> BoxFuture<'_, Result<ForkOutcome, String>>;
    /// `switchSession(sessionPath, options)`; `cwd_override` models the
    /// missing-cwd retry.
    fn switch_session(
        &self,
        session_path: &str,
        cwd_override: Option<&str>,
    ) -> BoxFuture<'_, Result<ForkOutcome, HostError>>;
    /// `importFromJsonl(path, cwdOverride?)`.
    fn import_from_jsonl(
        &self,
        path: &str,
        cwd_override: Option<&str>,
    ) -> BoxFuture<'_, Result<ForkOutcome, HostError>>;
    /// `runtimeHost.services.agentDir`.
    fn agent_dir(&self) -> String;
}

/// Host failures that branch in the shell (upstream error instanceof checks).
#[derive(Debug, Clone, PartialEq)]
pub enum HostError {
    MissingSessionCwd { fallback_cwd: String },
    ImportFileNotFound(String),
    Other(String),
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingSessionCwd { .. } => f.write_str("missing session cwd"),
            Self::ImportFileNotFound(m) => f.write_str(m),
            Self::Other(m) => f.write_str(m),
        }
    }
}

/// Upstream `fork` command-context result projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkOutcome {
    pub cancelled: bool,
    pub selected_text: Option<String>,
}

/// Upstream `AgentSessionRuntime.newSession` result (`{ cancelled: boolean }`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSessionOutcome {
    pub cancelled: bool,
}

/// The lower-half command handlers (S3). The r18 shell decides when each
/// fires; the r19 slice implements the bodies.
pub trait CommandSink: Send + Sync {
    fn run(&self, command: ShellCommand);
}

/// One upstream command invocation (`cmd.<name>` in the oracle log).
#[derive(Debug, Clone, PartialEq)]
pub enum ShellCommand {
    Settings,
    ScopedModels,
    Model(Option<String>),
    Thinking(Option<String>),
    Export(String),
    Import(String),
    Share,
    Bug(Option<String>),
    Copy {
        flash_confirmation: bool,
        prefer_selection: bool,
    },
    Name(String),
    Session,
    Changelog,
    Hotkeys,
    UserMessageSelector,
    Clone,
    Tree,
    Trust,
    Login(Option<String>),
    OAuthLogout,
    Clear,
    Compact(Option<String>),
    Reload,
    Debug,
    ArminSaysHi,
    DementedDelves,
    SessionSelector,
    Bash {
        command: String,
        exclude_from_context: bool,
    },
    TreeSelector,
    ModelSelector,
}

impl ShellCommand {
    /// The oracle `cmd.<name>` key for this invocation.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Settings => "showSettingsSelector",
            Self::ScopedModels => "showModelsSelector",
            Self::Model(_) => "handleModelCommand",
            Self::Thinking(_) => "handleThinkingCommand",
            Self::Export(_) => "handleExportCommand",
            Self::Import(_) => "handleImportCommand",
            Self::Share => "handleShareCommand",
            Self::Bug(_) => "handleBugCommand",
            Self::Copy { .. } => "handleCopyCommand",
            Self::Name(_) => "handleNameCommand",
            Self::Session => "handleSessionCommand",
            Self::Changelog => "handleChangelogCommand",
            Self::Hotkeys => "handleHotkeysCommand",
            Self::UserMessageSelector => "showUserMessageSelector",
            Self::Clone => "handleCloneCommand",
            Self::Tree => "showTreeSelector",
            Self::Trust => "showTrustSelector",
            Self::Login(_) => "handleLoginCommand",
            Self::OAuthLogout => "showOAuthSelector",
            Self::Clear => "handleClearCommand",
            Self::Compact(_) => "handleCompactCommand",
            Self::Reload => "handleReloadCommand",
            Self::Debug => "handleDebugCommand",
            Self::ArminSaysHi => "handleArminSaysHi",
            Self::DementedDelves => "handleDementedDelves",
            Self::SessionSelector => "showSessionSelector",
            Self::Bash { .. } => "handleBashCommand",
            Self::TreeSelector => "showTreeSelector",
            Self::ModelSelector => "showModelSelector",
        }
    }
}

/// The application action names the input ring registers (upstream
/// `AppKeybinding` values, in registration order).
pub const INPUT_RING_ACTIONS: [&str; 16] = [
    "app.clear",
    "app.suspend",
    "app.thinking.cycle",
    "app.model.cycleForward",
    "app.model.cycleBackward",
    "app.model.select",
    "app.tools.expand",
    "app.thinking.toggle",
    "app.editor.external",
    "app.message.copy",
    "app.message.followUp",
    "app.message.dequeue",
    "app.session.new",
    "app.session.tree",
    "app.session.fork",
    "app.session.resume",
];

// ===========================================================================
// r20 lower half: model / resource / platform projections
// ===========================================================================

/// The model shape the shell's decisions consume (upstream `Model<any>`
/// reads: `provider`/`id`/`name`/`api`/`reasoning`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelRef {
    pub provider: String,
    pub id: String,
    pub name: Option<String>,
    /// Upstream `model.api` (the `"unknown"` triple marks an unselected model).
    pub api: Option<String>,
    pub reasoning: bool,
}

impl ModelRef {
    /// Upstream `isUnknownModel(model)`.
    pub fn is_unknown(&self) -> bool {
        is_unknown_model(Some(&self.provider), Some(&self.id), self.api.as_deref())
    }
    /// `provider/id` reference used by the settings selector footer lines and
    /// the model-scope helpers.
    pub fn reference(&self) -> String {
        format!("{}/{}", self.provider, self.id)
    }
    /// Describe projection for the oracle tuples (name omitted when unset).
    pub fn to_value(&self) -> Value {
        let mut out = serde_json::Map::new();
        out.insert("provider".into(), json_value(&self.provider));
        out.insert("id".into(), json_value(&self.id));
        out.insert(
            "name".into(),
            match &self.name {
                Some(name) => json_value(name),
                None => Value::Null,
            },
        );
        out.insert("reasoning".into(), Value::Bool(self.reasoning));
        Value::Object(out)
    }
}

fn json_value(s: &str) -> Value {
    Value::String(s.to_string())
}

/// Upstream `AuthSelectorProvider` with the method/status payload the login
/// dialogs read.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AuthProviderOption {
    pub id: String,
    pub name: String,
    /// `"oauth" | "api_key"`.
    pub auth_type: String,
    /// The login-capable method marker (`provider.auth.oauth` / `.apiKey`).
    pub method_login: bool,
    /// `method.name` (ambient dialog headline).
    pub method_name: Option<String>,
    /// `method.loginLabel` (subscription auth-type row label).
    pub login_label: Option<String>,
    /// `(type, source)` when the provider is configured.
    pub status: Option<(String, Option<String>)>,
    /// Whether the OAuth sign-in is backed by a subscription (v1.0.0
    /// `provider.auth.oauth.isSubscription === true`). `None` keeps the
    /// "subscription" label of the captured oracles; upstream marks plain
    /// accounts with `false` instead.
    pub subscription: Option<bool>,
}

/// Upstream `getUsageCostBreakdown` row.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UsageCostRow {
    pub key: String,
    pub cost: f64,
    pub tokens: u64,
}

/// Upstream `computeCacheWaste` projection.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CacheWaste {
    pub missed_tokens: u64,
    pub missed_cost: f64,
    pub miss_count: u64,
}

/// Upstream session stats (`session.getSessionStats()`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SessionStats {
    pub session_file: Option<String>,
    pub session_id: String,
    pub total_messages: u64,
    pub user_messages: u64,
    pub assistant_messages: u64,
    pub tool_calls: u64,
    pub tool_results: u64,
    pub tokens_input: u64,
    pub tokens_output: u64,
    pub tokens_cache_read: u64,
    pub tokens_cache_write: u64,
    pub tokens_total: u64,
    pub cost: f64,
}

/// Upstream `session.getUserMessagesForForking()` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkableUserMessage {
    pub entry_id: String,
    pub text: String,
}

/// Upstream `navigateTree` result projection.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NavigateOutcome {
    pub cancelled: bool,
    pub aborted: bool,
    pub editor_text: Option<String>,
}

/// The `buildBugReportBundle` outcome projection: what the flow's statuses
/// and `recordInSession` need (upstream works on the full `BugReportBundle`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BugReportOutcome {
    /// `bundle.metadata.id`.
    pub report_id: String,
    /// `bundle.metadata.createdAt`.
    pub created_at: String,
    /// The zip archive path (`delivery: "zip"`; `None` for upload).
    pub zip_path: Option<String>,
    /// `bundle.diagnostics.crashes.length` (drives the crash-log clear).
    pub crash_count: usize,
}

/// Upstream `session.executeBash` result projection.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BashOutcome {
    pub exit_code: Option<i64>,
    pub cancelled: bool,
    pub output: String,
    pub truncated: bool,
    pub full_output_path: Option<String>,
}

/// Upstream `user_bash` extension interception result.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UserBashOutcome {
    /// A full extension-provided result short-circuits local execution.
    pub result: Option<BashOutcome>,
}

/// The model-runtime surface the lower half reads (upstream
/// `session.modelRuntime.*`).
pub trait ShellModelRuntime: Send + Sync {
    fn available_snapshot(&self) -> Vec<ModelRef>;
    fn providers(&self) -> Vec<Value>;
    fn provider_auth_status(&self, provider_id: &str) -> (bool, Option<String>, Option<String>);
    fn is_using_oauth(&self, provider_id: &str) -> bool;
    fn check_auth(&self, provider_id: &str) -> BoxFuture<'_, Option<String>>;
    fn get_auth_api_key(&self, provider_id: &str) -> BoxFuture<'_, Option<String>>;
    fn list_credentials(&self) -> BoxFuture<'_, Result<Vec<Value>, String>>;
    /// `getProvider(providerId)` — the stored-credential name lookup of
    /// `getLogoutProviderOptions` (`getProvider(providerId)?.name ??
    /// providerId`). `Err` models a runtime whose `getProvider` member is
    /// missing (the JS harness stub): the upstream member call throws
    /// `this.session.modelRuntime.getProvider is not a function`, which the
    /// shell surfaces through the same catch as any other lookup failure.
    fn get_provider_name(&self, provider_id: &str) -> Result<Option<String>, String>;
    fn logout(&self, provider_id: &str) -> BoxFuture<'_, Result<(), String>>;
    /// `login(providerId, method, { signal, prompt, notify })` — the prompt /
    /// notify callbacks travel as enum payloads (S8).
    fn login(
        &self,
        provider_id: &str,
        method: &str,
        callbacks: LoginCallbacks<'_>,
    ) -> BoxFuture<'_, Result<(), LoginError>>;
    fn refresh(&self, providers: Option<Vec<String>>) -> BoxFuture<'_, RefreshResult>;
    fn get_error(&self) -> Option<String>;
}

/// The prompt callback of `modelRuntime.login`.
pub enum LoginPrompt {
    Select {
        message: String,
        options: Vec<(String, String)>,
    },
    ManualCode {
        message: String,
    },
    Prompt {
        message: String,
        placeholder: Option<String>,
    },
    /// Already aborted.
    Aborted,
}

impl LoginPrompt {
    pub fn to_value(&self) -> Value {
        match self {
            Self::Select { message, options } => serde_json::json!({
                "type": "select",
                "message": message,
                "options": options.iter().map(|(id, label)| serde_json::json!({
                    "id": id, "label": label,
                })).collect::<Vec<_>>(),
            }),
            Self::ManualCode { message } => {
                serde_json::json!({ "type": "manual_code", "message": message })
            }
            Self::Prompt {
                message,
                placeholder,
            } => serde_json::json!({
                "type": "prompt",
                "message": message,
                "placeholder": placeholder,
            }),
            Self::Aborted => Value::Null,
        }
    }
}

/// The notify callback payload of `modelRuntime.login`.
#[derive(Debug, Clone, PartialEq)]
pub enum LoginNotify {
    AuthUrl {
        url: String,
        instructions: Option<String>,
    },
    DeviceCode(Value),
    Info {
        message: String,
        links: Value,
    },
    Progress {
        message: String,
    },
}

/// Callback bundle for `modelRuntime.login` (S8).
pub struct LoginCallbacks<'a> {
    pub prompt:
        &'a (dyn Fn(LoginPrompt) -> BoxFuture<'static, Result<String, String>> + Send + Sync),
    pub notify: &'a (dyn Fn(LoginNotify) + Send + Sync),
}

impl<'a> LoginCallbacks<'a> {
    pub fn bounded(
        prompt: &'a (dyn Fn(LoginPrompt) -> BoxFuture<'static, Result<String, String>>
                 + Send
                 + Sync),
        notify: &'a (dyn Fn(LoginNotify) + Send + Sync),
    ) -> Self {
        Self { prompt, notify }
    }
}

/// `login` failure (upstream rejects with `Error`; `"Login cancelled"` is
/// matched by message).
#[derive(Debug, Clone, PartialEq)]
pub struct LoginError(pub String);

impl std::fmt::Display for LoginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// `modelRuntime.refresh` result projection.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RefreshResult {
    pub aborted: bool,
    /// Refreshed provider errors (`provider → message`).
    pub errors: Vec<String>,
}

/// The resource-loader read surface for `showLoadedResources` (upstream
/// `session.resourceLoader.*`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LoadedResource {
    pub name: Option<String>,
    pub path: String,
    pub source_info: Option<SourceInfoView>,
    /// Loaded-theme `sourcePath` (themes only).
    pub source_path: Option<String>,
    /// Extension `hidden` flag (upstream filters hidden rows from the
    /// loaded-resources listing).
    pub hidden: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ResourceGroupRead {
    pub items: Vec<LoadedResource>,
    pub diagnostics: Vec<ResourceDiagnostic>,
}

/// The resource-loader surface the shell reads.
pub trait ShellResources: Send + Sync {
    fn skills(&self) -> ResourceGroupRead;
    fn prompts(&self) -> ResourceGroupRead;
    fn themes(&self) -> ResourceGroupRead;
    fn extensions(&self) -> (Vec<LoadedResource>, Vec<(String, String)>);
    fn system_prompt_source(&self) -> Option<LoadedResource>;
    fn append_system_prompt_sources(&self) -> Vec<LoadedResource>;
    fn agents_files(&self) -> Vec<LoadedResource>;
    fn prompt_templates(&self) -> Vec<LoadedResource>;
}

/// The project-trust store surface (upstream `ProjectTrustStore`).
pub trait ShellTrustStore: Send + Sync {
    fn get(&self, cwd: &str) -> BoxFuture<'_, Option<bool>>;
    fn get_entry(&self, cwd: &str) -> Value;
    fn set(&self, cwd: &str, trusted: bool);
    fn set_many(&self, updates: Value);
}

/// The platform/process seam (S7): exit, signals, suspend, clipboard,
/// telemetry-offline gates. Recording implementations capture the calls.
pub trait ShellPlatform: Send + Sync {
    /// `process.exit(code)` — returns whether the (test-double) process
    /// survived the exit call. Real implementations never return (`false`
    /// would be unreachable); oracle drivers model the harness stub
    /// semantics, where a fake exit records and falls through.
    fn exit(&self, code: i32) -> bool;
    /// `process.platform === "win32"`.
    fn is_windows(&self) -> bool;
    /// `Date.now()`.
    fn now_ms(&self) -> i64;
    /// `process.stdout.isTTY` probe of `formatResumeCommand`.
    fn stdout_is_tty(&self) -> bool;
    /// `killTrackedDetachedChildren`.
    fn kill_tracked_detached_children(&self);
    /// `copyToClipboard`.
    fn copy_to_clipboard(&self, text: &str) -> BoxFuture<'_, Result<(), String>>;
    /// `readClipboardText`/`readClipboardImage` (image → `(mime, bytes)`).
    fn read_clipboard_text(&self) -> BoxFuture<'_, Option<String>>;
    fn read_clipboard_image(&self) -> BoxFuture<'_, Option<(String, Vec<u8>)>>;
    /// `readClipboardFilePaths` (delta: files copied to the clipboard paste
    /// their original paths). `None` falls through to the image/text probes.
    fn read_clipboard_file_paths(&self) -> BoxFuture<'_, Option<Vec<String>>> {
        Box::pin(async { None })
    }
    /// `PI_OFFLINE`.
    fn pi_offline(&self) -> bool;
    /// The trusted-project resource probe (`hasTrustRequiringProjectResources`).
    fn has_trust_requiring_project_resources(&self, cwd: &str) -> bool;
    /// `ensureTool`/`loadAllHighlightLanguages` choreography — presentation;
    /// the shell records the startup completion only.
    fn register_signal_handlers(&self) -> Vec<u64>;
    fn unregister_signal_handlers(&self, ids: &[u64]);
    /// Suspend choreography (posix only); `None` when unsupported.
    fn suspend(&self) -> Option<()>;
    /// `new Date().toISOString()`.
    fn now_iso(&self) -> String;
    /// `path.basename`.
    fn basename(&self, path: &str) -> String;
    /// `path.join`.
    fn join_path(&self, parts: &[&str]) -> String;
    /// `fs.existsSync`.
    fn file_exists(&self, path: &str) -> bool;
    /// The module-level `stopThemeWatcher()` of `handleFatalRuntimeError`;
    /// `Err` models the recording harness where the symbol is absent (the
    /// ReferenceError unwinds the rest of the body).
    fn stop_theme_watcher(&self) -> Result<(), String> {
        Ok(())
    }
}

/// Extension shortcuts (`extensionRunner.getShortcuts(config)`).
#[derive(Debug, Clone, PartialEq)]
pub struct ExtensionShortcut {
    /// The key id string (`ctrl+shift+g`).
    pub key: String,
    pub description: Option<String>,
    pub extension_path: String,
}

/// Extra extension-runner surface used by the lower half.
pub trait ShellShortcutSurface: Send + Sync {
    fn shortcuts(&self) -> Vec<ExtensionShortcut>;
    /// `emitUserBash`.
    fn emit_user_bash(
        &self,
        command: &str,
        exclude_from_context: bool,
        cwd: &str,
    ) -> BoxFuture<'_, Result<UserBashOutcome, String>>;
}
