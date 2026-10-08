//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/components/footer.ts` (245 lines, sha256
//! `176d2473ccefbc5ff2bf56d986e600a00d5841c38c5854c4c16ecb05d7e70499`): the
//! footer line — pwd (+ branch/session), usage stats, context %, model and
//! extension statuses.
//!
//! The session/provider surface is abstracted behind [`FooterSession`] and
//! [`FooterDataProvider`] (upstream reads `AgentSession` +
//! `ReadonlyFooterDataProvider` fields directly; the trait keeps the component
//! testable and mirrors the exact reads). `core/usage-totals.ts` is re-stated
//! locally ([`UsageTotals`], the same accumulation the ported
//! `agent_session` uses privately).

use std::collections::BTreeMap;

use crate::coding_agent::cli::startup_ui::are_experimental_features_enabled;
use crate::coding_agent::modes::interactive::components::model_selector::theme_fg;
use crate::coding_agent::modes::interactive::theme::Theme;
use crate::tui::utils::{truncate_to_width, visible_width};

/// Sanitize text for display in a single-line status
/// (upstream `sanitizeStatusText`).
fn sanitize_status_text(text: &str) -> String {
    let replaced: String = text
        .chars()
        .map(|c| {
            if matches!(c, '\r' | '\n' | '\t') {
                ' '
            } else {
                c
            }
        })
        .collect();
    // collapse runs of spaces, then trim
    let mut collapsed = String::with_capacity(replaced.len());
    let mut last_was_space = false;
    for c in replaced.chars() {
        if c == ' ' {
            if !last_was_space {
                collapsed.push(' ');
            }
            last_was_space = true;
        } else {
            collapsed.push(c);
            last_was_space = false;
        }
    }
    collapsed.trim().to_string()
}

/// Format token counts for compact footer display (upstream `formatTokens`).
pub fn format_tokens(count: u64) -> String {
    let count_f = count as f64;
    if count < 1000 {
        count.to_string()
    } else if count < 10_000 {
        format!("{:.1}k", count_f / 1000.0)
    } else if count < 1_000_000 {
        format!("{}k", (count_f / 1000.0).round() as u64)
    } else if count < 10_000_000 {
        format!("{:.1}M", count_f / 1_000_000.0)
    } else {
        format!("{}M", (count_f / 1_000_000.0).round() as u64)
    }
}

/// Replace the home directory prefix with `~` (upstream `formatCwdForFooter`).
/// `resolve`/`relative` mirror node's path semantics for the host platform.
pub fn format_cwd_for_footer(cwd: &str, home: Option<&str>) -> String {
    let Some(home) = home else {
        return cwd.to_string();
    };
    let resolved_cwd = resolve_path(cwd);
    let resolved_home = resolve_path(home);
    let relative_to_home = relative_path(&resolved_home, &resolved_cwd);
    let sep = std::path::MAIN_SEPARATOR;
    let is_inside_home = relative_to_home.is_empty()
        || (relative_to_home != ".."
            && !relative_to_home.starts_with(&format!("..{sep}"))
            && !std::path::Path::new(&relative_to_home).is_absolute());
    if !is_inside_home {
        return cwd.to_string();
    }
    if relative_to_home.is_empty() {
        "~".to_string()
    } else {
        format!("~{sep}{relative_to_home}")
    }
}

fn resolve_path(path: &str) -> String {
    // node `path.resolve` for an absolute input returns it normalized; the
    // footer inputs are absolute on the host.
    if std::path::Path::new(path).is_absolute() {
        normalize_separators(path)
    } else {
        normalize_separators(&format!(
            "{}{}{path}",
            std::env::current_dir()
                .map(|d| d.to_string_lossy().into_owned())
                .unwrap_or_default(),
            std::path::MAIN_SEPARATOR
        ))
    }
}

fn normalize_separators(path: &str) -> String {
    // collapse duplicate separators (node path.resolve normalization)
    let mut out = String::with_capacity(path.len());
    let mut last_sep = false;
    for c in path.chars() {
        if c == '/' || c == '\\' {
            if !last_sep {
                out.push(std::path::MAIN_SEPARATOR);
            }
            last_sep = true;
        } else {
            out.push(c);
            last_sep = false;
        }
    }
    out
}

fn relative_path(from: &str, to: &str) -> String {
    // node `path.relative` for paths sharing the `from` prefix (the footer's
    // only use); otherwise the destination is returned.
    let from_parts: Vec<&str> = from
        .split(std::path::MAIN_SEPARATOR)
        .filter(|p| !p.is_empty())
        .collect();
    let to_parts: Vec<&str> = to
        .split(std::path::MAIN_SEPARATOR)
        .filter(|p| !p.is_empty())
        .collect();
    let mut common = 0usize;
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
        result.push(part.to_string());
    }
    if result.is_empty() {
        String::new()
    } else {
        result.join(std::path::MAIN_SEPARATOR_STR)
    }
}

/// One usage snapshot (upstream `Usage` subset the footer reads).
#[derive(Clone, Copy, Debug, Default)]
pub struct FooterUsage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost: f64,
}

/// Upstream `SessionEntry` slices the footer reads.
#[derive(Clone, Debug)]
pub enum FooterEntry {
    AssistantMessage(FooterUsage),
    ToolResultMessage(FooterUsage),
    BranchSummary(FooterUsage),
    Compaction(FooterUsage),
    /// `usage` entries (model-attributed usage outside the LLM context, e.g.
    /// cache warming). Counted toward the totals like every other usage.
    Usage(FooterUsage),
    Other,
}

/// Upstream `state.model` slice.
#[derive(Clone, Debug)]
pub struct FooterModel {
    pub id: String,
    pub provider: String,
    pub context_window: u64,
    pub reasoning: bool,
}

/// Upstream `session.routedModel` slice (`{ model, thinkingLevel? }`).
#[derive(Clone, Debug)]
pub struct FooterRoutedModel {
    pub model: FooterModel,
    pub thinking_level: Option<String>,
}

/// The `AgentSession` reads the footer performs (upstream accesses the fields
/// directly).
pub trait FooterSession {
    fn state_model(&self) -> Option<FooterModel>;
    fn state_thinking_level(&self) -> Option<String>;
    /// `(contextWindow, percent)` — percent `None` renders `?`.
    fn context_usage(&self) -> Option<(u64, Option<f64>)>;
    fn cwd(&self) -> String;
    fn session_name(&self) -> Option<String>;
    fn entries(&self) -> Vec<FooterEntry>;
    fn is_using_subscription(&self, provider: &str) -> bool;
    /// `sessionManager.getSessionId()`.
    fn session_id(&self) -> String;
    /// `sessionManager.getLeafId()`.
    fn leaf_id(&self) -> Option<String>;
    /// `sessionManager.getEntryCount()`.
    fn entry_count(&self) -> usize;
    /// `session.routedModel` — the physical model of the latest response
    /// under a virtual selection.
    fn routed_model(&self) -> Option<FooterRoutedModel>;
}

/// The `ReadonlyFooterDataProvider` reads.
pub trait FooterDataProvider {
    fn git_branch(&self) -> Option<String>;
    fn extension_statuses(&self) -> BTreeMap<String, String>;
    fn available_provider_count(&self) -> usize;
}

/// Upstream `createUsageTotals` + `addUsageToTotals` (core/usage-totals.ts).
#[derive(Clone, Copy, Debug, Default)]
pub struct UsageTotals {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost: f64,
}

/// Upstream `SessionStats` — the per-render scan result of
/// `getSessionStats()`.
#[derive(Clone, Debug)]
struct SessionStats {
    usage_totals: UsageTotals,
    latest_cache_hit_rate: Option<f64>,
    context_usage: Option<(u64, Option<f64>)>,
    session_id: String,
    leaf_id: Option<String>,
    entry_count: usize,
    /// The cache key (upstream compares `sessionId`, `leafId`, `entryCount`,
    /// and `limitsModel` identity; the port compares the model's value
    /// tuple, which is behaviorally equivalent for the footer's reads).
    limits_model_key: Option<(String, String, u64, bool)>,
}

/// Upstream `FooterComponent`.
pub struct FooterComponent<'a> {
    auto_compact_enabled: bool,
    session: &'a dyn FooterSession,
    footer_data: &'a dyn FooterDataProvider,
    session_stats: std::cell::RefCell<Option<SessionStats>>,
}

impl<'a> FooterComponent<'a> {
    /// Upstream constructor.
    pub fn new(session: &'a dyn FooterSession, footer_data: &'a dyn FooterDataProvider) -> Self {
        Self {
            auto_compact_enabled: true,
            session,
            footer_data,
            session_stats: std::cell::RefCell::new(None),
        }
    }

    /// Upstream `setAutoCompactEnabled`.
    pub fn set_auto_compact_enabled(&mut self, enabled: bool) {
        self.auto_compact_enabled = enabled;
    }

    /// Upstream `getSessionStats`: usage totals and context usage scan the
    /// whole session, and the footer renders on every frame. Entries are
    /// append-only and every append moves the leaf, so the results only
    /// change with the session, leaf, entry count, or the model whose
    /// context window applies.
    fn get_session_stats(&self) -> SessionStats {
        let entry_count = self.session.entry_count();
        let session_id = self.session.session_id();
        let leaf_id = self.session.leaf_id();
        // Upstream `this.session.routedModel?.model ?? this.session.model`.
        let limits_model_key = self
            .session
            .routed_model()
            .map(|routed| {
                (
                    routed.model.provider,
                    routed.model.id,
                    routed.model.context_window,
                    routed.model.reasoning,
                )
            })
            .or_else(|| {
                self.session
                    .state_model()
                    .map(|m| (m.provider, m.id, m.context_window, m.reasoning))
            });
        if let Some(cached) = self.session_stats.borrow().as_ref() {
            if cached.session_id == session_id
                && cached.leaf_id == leaf_id
                && cached.entry_count == entry_count
                && cached.limits_model_key == limits_model_key
            {
                return cached.clone();
            }
        }

        // Calculate cumulative usage from ALL session entries.
        let mut usage_totals = UsageTotals::default();
        let mut latest_cache_hit_rate: Option<f64> = None;

        for entry in self.session.entries() {
            match entry {
                FooterEntry::Usage(usage) => add_usage(&mut usage_totals, &usage),
                FooterEntry::AssistantMessage(usage) => {
                    add_usage(&mut usage_totals, &usage);
                    let latest_prompt_tokens = usage.input + usage.cache_read + usage.cache_write;
                    latest_cache_hit_rate = if latest_prompt_tokens > 0 {
                        Some(usage.cache_read as f64 / latest_prompt_tokens as f64 * 100.0)
                    } else {
                        None
                    };
                }
                FooterEntry::ToolResultMessage(usage) => add_usage(&mut usage_totals, &usage),
                FooterEntry::BranchSummary(usage) | FooterEntry::Compaction(usage) => {
                    add_usage(&mut usage_totals, &usage)
                }
                FooterEntry::Other => {}
            }
        }

        // Context usage (handles compaction correctly).
        let context_usage = self.session.context_usage();
        let stats = SessionStats {
            usage_totals,
            latest_cache_hit_rate,
            context_usage,
            limits_model_key,
            session_id,
            leaf_id,
            entry_count,
        };
        *self.session_stats.borrow_mut() = Some(stats.clone());
        stats
    }

    /// Upstream `render`.
    pub fn render_footer(&self, width: usize, theme: &Theme) -> Vec<String> {
        let stats = self.get_session_stats();
        let usage_totals = stats.usage_totals;
        let latest_cache_hit_rate = stats.latest_cache_hit_rate;

        let context_usage = stats.context_usage;
        let model = self.session.state_model();
        let context_window = context_usage
            .map(|(w, _)| w)
            .or(model.as_ref().map(|m| m.context_window))
            .unwrap_or(0);
        let context_percent_value = context_usage.map(|(_, p)| p.unwrap_or(0.0)).unwrap_or(0.0);
        // upstream `contextUsage?.percent !== null ? toFixed(1) : "?"`: a null
        // contextUsage renders "0.0"; only an explicit null percent renders "?".
        let context_percent = match context_usage {
            Some((_, None)) => "?".to_string(),
            _ => format!("{context_percent_value:.1}"),
        };

        // Replace home directory with ~
        let home = std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok());
        let mut pwd = format_cwd_for_footer(&self.session.cwd(), home.as_deref());

        if let Some(branch) = self.footer_data.git_branch() {
            pwd = format!("{pwd} ({branch})");
        }
        if let Some(session_name) = self.session.session_name() {
            pwd = format!("{pwd} • {session_name}");
        }

        // Build stats line
        let mut stats_parts: Vec<String> = Vec::new();
        if usage_totals.input > 0 {
            stats_parts.push(format!("↑{}", format_tokens(usage_totals.input)));
        }
        if usage_totals.output > 0 {
            stats_parts.push(format!("↓{}", format_tokens(usage_totals.output)));
        }
        if usage_totals.cache_read > 0 {
            stats_parts.push(format!("R{}", format_tokens(usage_totals.cache_read)));
        }
        if usage_totals.cache_write > 0 {
            stats_parts.push(format!("W{}", format_tokens(usage_totals.cache_write)));
        }
        if (usage_totals.cache_read > 0 || usage_totals.cache_write > 0)
            && latest_cache_hit_rate.is_some()
        {
            stats_parts.push(format!("CH{:.1}%", latest_cache_hit_rate.unwrap_or(0.0)));
        }

        // Kimi Coding is subscription-backed despite using API-key auth.
        let using_subscription = match &model {
            Some(model) => {
                model.provider == "kimi-coding"
                    || self.session.is_using_subscription(&model.provider)
            }
            None => false,
        };
        if usage_totals.cost != 0.0 || using_subscription {
            let cost_str = format!(
                "${:.3}{}",
                usage_totals.cost,
                if using_subscription { " (sub)" } else { "" }
            );
            stats_parts.push(cost_str);
        }

        let auto_indicator = if self.auto_compact_enabled {
            " (auto)"
        } else {
            ""
        };
        let context_percent_display = if context_percent == "?" {
            format!("?/{}{auto_indicator}", format_tokens(context_window))
        } else {
            format!(
                "{context_percent}%/{}{auto_indicator}",
                format_tokens(context_window)
            )
        };
        let context_percent_str = if context_percent_value > 90.0 {
            theme_fg(theme, "error", &context_percent_display)
        } else if context_percent_value > 70.0 {
            theme_fg(theme, "warning", &context_percent_display)
        } else {
            context_percent_display.clone()
        };
        stats_parts.push(context_percent_str);
        if are_experimental_features_enabled() {
            stats_parts.push(format!(
                "{} {}",
                theme_fg(theme, "dim", "•"),
                theme.bold(&theme_fg(theme, "warning", "xp"))
            ));
        }

        let mut stats_left = stats_parts.join(" ");

        let model_name = model
            .as_ref()
            .map(|m| m.id.clone())
            .unwrap_or_else(|| "no-model".to_string());
        let mut stats_left_width = visible_width(&stats_left);

        if stats_left_width > width {
            stats_left = truncate_to_width(&stats_left, width, "...", false);
            stats_left_width = visible_width(&stats_left);
        }

        let min_padding = 2usize;

        let mut right_side_without_provider = model_name.clone();
        if model.as_ref().is_some_and(|m| m.reasoning) {
            let thinking_level = self
                .session
                .state_thinking_level()
                .unwrap_or_else(|| "off".to_string());
            right_side_without_provider = if thinking_level == "off" {
                format!("{model_name} • thinking off")
            } else {
                format!("{model_name} • {thinking_level}")
            };
        }
        // A virtual model routes each request; show where the latest response went.
        if let Some(routed) = self.session.routed_model() {
            let level = routed
                .thinking_level
                .as_deref()
                .map(|level| format!(" • {level}"))
                .unwrap_or_default();
            right_side_without_provider.push_str(&format!(" → {}{level}", routed.model.id));
        }

        let mut right_side = right_side_without_provider.clone();
        if self.footer_data.available_provider_count() > 1 {
            if let Some(model) = &model {
                right_side = format!("({}) {right_side_without_provider}", model.provider);
                if stats_left_width + min_padding + visible_width(&right_side) > width {
                    right_side = right_side_without_provider.clone();
                }
            }
        }

        let right_side_width = visible_width(&right_side);
        let total_needed = stats_left_width + min_padding + right_side_width;

        let stats_line = if total_needed <= width {
            let padding = " ".repeat(width - stats_left_width - right_side_width);
            format!("{stats_left}{padding}{right_side}")
        } else {
            let available_for_right =
                width as isize - stats_left_width as isize - min_padding as isize;
            if available_for_right > 0 {
                let truncated_right =
                    truncate_to_width(&right_side, available_for_right as usize, "", false);
                let truncated_right_width = visible_width(&truncated_right);
                let padding =
                    " ".repeat(width.saturating_sub(stats_left_width + truncated_right_width));
                format!("{stats_left}{padding}{truncated_right}")
            } else {
                stats_left.clone()
            }
        };

        // Dim each part separately so inner color codes survive.
        let dim_stats_left = theme_fg(theme, "dim", &stats_left);
        let remainder = stats_line[stats_left.len()..].to_string();
        let dim_remainder = theme_fg(theme, "dim", &remainder);

        let pwd_line = truncate_to_width(
            &theme_fg(theme, "dim", &pwd),
            width,
            &theme_fg(theme, "dim", "..."),
            false,
        );
        let mut lines = vec![pwd_line, format!("{dim_stats_left}{dim_remainder}")];

        // Extension statuses, sorted by key alphabetically.
        let extension_statuses = self.footer_data.extension_statuses();
        if !extension_statuses.is_empty() {
            let sorted: BTreeMap<String, String> = extension_statuses;
            let status_line = sorted
                .values()
                .map(|text| sanitize_status_text(text))
                .collect::<Vec<_>>()
                .join(" ");
            lines.push(truncate_to_width(
                &status_line,
                width,
                &theme_fg(theme, "dim", "..."),
                false,
            ));
        }

        lines
    }
}

fn add_usage(totals: &mut UsageTotals, usage: &FooterUsage) {
    totals.input += usage.input;
    totals.output += usage.output;
    totals.cache_read += usage.cache_read;
    totals.cache_write += usage.cache_write;
    totals.cost += usage.cost;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent::modes::interactive::theme::{load_builtin_theme, ColorMode};
    use std::sync::Arc;

    fn dark() -> Arc<Theme> {
        Arc::new(load_builtin_theme("dark", Some(ColorMode::Truecolor)).expect("dark"))
    }

    #[derive(Clone)]
    struct FixedSession {
        model: Option<FooterModel>,
        thinking_level: Option<String>,
        context_usage: Option<(u64, Option<f64>)>,
        cwd: String,
        session_name: Option<String>,
        entries: Vec<FooterEntry>,
        subscription_providers: Vec<String>,
        routed: Option<FooterRoutedModel>,
        entry_count: usize,
    }

    impl FooterSession for FixedSession {
        fn state_model(&self) -> Option<FooterModel> {
            self.model.clone()
        }
        fn state_thinking_level(&self) -> Option<String> {
            self.thinking_level.clone()
        }
        fn context_usage(&self) -> Option<(u64, Option<f64>)> {
            self.context_usage
        }
        fn cwd(&self) -> String {
            self.cwd.clone()
        }
        fn session_name(&self) -> Option<String> {
            self.session_name.clone()
        }
        fn entries(&self) -> Vec<FooterEntry> {
            self.entries.clone()
        }
        fn is_using_subscription(&self, provider: &str) -> bool {
            self.subscription_providers.iter().any(|p| p == provider)
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

    struct FixedProvider {
        branch: Option<String>,
        statuses: BTreeMap<String, String>,
        provider_count: usize,
    }

    impl FooterDataProvider for FixedProvider {
        fn git_branch(&self) -> Option<String> {
            self.branch.clone()
        }
        fn extension_statuses(&self) -> BTreeMap<String, String> {
            self.statuses.clone()
        }
        fn available_provider_count(&self) -> usize {
            self.provider_count
        }
    }

    fn usage(input: u64, output: u64, cache_read: u64, cache_write: u64, cost: f64) -> FooterUsage {
        FooterUsage {
            input,
            output,
            cache_read,
            cache_write,
            cost,
        }
    }

    /// Oracle: tests/fixtures/interactive_r19_oracle scenario `footer` —
    /// formatTokens/cwd arrays byte-exact; renders byte-exact.
    #[test]
    fn format_tokens_and_cwd_match_oracle() {
        let expected = [
            "0", "999", "1.0k", "1.2k", "10.0k", "10k", "1000k", "1.0M", "1.2M", "10.0M", "10M",
        ];
        for (value, want) in [
            0u64, 999, 1000, 1234, 9999, 10_000, 999_999, 1_000_000, 1_234_567, 9_999_999,
            10_000_000,
        ]
        .into_iter()
        .zip(expected)
        {
            assert_eq!(format_tokens(value), want);
        }
        // environment-anchored: the cwd/home anchors are host-shaped inputs.
        // The capture pins upstream-on-win32 (`path.sep` = `\`); on POSIX the
        // same scenarios use POSIX-shaped anchors and upstream node renders
        // them with `/` (its `path.resolve`/`relative`/`sep` are
        // platform-dispatched), so the expectations are split with the input.
        if cfg!(windows) {
            assert_eq!(
                format_cwd_for_footer("C:\\Users\\n\\proj", Some("C:\\Users\\n")),
                "~\\proj"
            );
            assert_eq!(
                format_cwd_for_footer("C:\\Users\\n\\proj", Some("C:\\Users\\m")),
                "C:\\Users\\n\\proj"
            );
            assert_eq!(
                format_cwd_for_footer("C:\\Users\\n", Some("C:\\Users\\n")),
                "~"
            );
            assert_eq!(format_cwd_for_footer("C:\\other", None), "C:\\other");
            assert_eq!(
                format_cwd_for_footer("C:\\Users\\n\\..\\elsewhere", Some("C:\\Users\\n")),
                "C:\\Users\\n\\..\\elsewhere"
            );
        } else {
            assert_eq!(
                format_cwd_for_footer("/home/n/proj", Some("/home/n")),
                "~/proj"
            );
            assert_eq!(
                format_cwd_for_footer("/home/n/proj", Some("/home/m")),
                "/home/n/proj"
            );
            assert_eq!(format_cwd_for_footer("/home/n", Some("/home/n")), "~");
            assert_eq!(format_cwd_for_footer("/other", None), "/other");
            assert_eq!(
                format_cwd_for_footer("/home/n/../elsewhere", Some("/home/n")),
                "/home/n/../elsewhere"
            );
        }
    }

    #[test]
    fn footer_render_matches_oracle() {
        let theme = dark();
        // environment-anchored: the fixture cwd must render raw (outside the
        // live HOME), which is only meaningful with a host-shaped absolute
        // path. The capture used a fake win32 profile path; on POSIX the fake
        // `C:\...` input is relative and would resolve under the live HOME
        // (upstream `resolve()` behaves identically), so mirror the scenario
        // with a POSIX-absolute anchor outside any plausible HOME.
        let cwd = if cfg!(windows) {
            "C:\\Users\\n\\proj".to_string()
        } else {
            "/pi-footer-oracle-cwd/proj".to_string()
        };
        let session = FixedSession {
            model: Some(FooterModel {
                id: "kimi-k2".to_string(),
                provider: "kimi-coding".to_string(),
                context_window: 100_000,
                reasoning: true,
            }),
            thinking_level: Some("high".to_string()),
            context_usage: Some((200_000, Some(42.55))),
            cwd,
            session_name: Some("my-session".to_string()),
            entries: vec![
                FooterEntry::AssistantMessage(usage(1500, 250, 100, 50, 0.5)),
                FooterEntry::ToolResultMessage(usage(10, 5, 0, 0, 0.05)),
                FooterEntry::BranchSummary(usage(100, 10, 0, 0, 0.01)),
            ],
            subscription_providers: Vec::new(),
            routed: None,
            entry_count: 3,
        };
        let provider = FixedProvider {
            branch: Some("main".to_string()),
            statuses: BTreeMap::from([
                ("b".to_string(), "second status".to_string()),
                ("a".to_string(), "first\tstatus".to_string()),
            ]),
            provider_count: 2,
        };
        let footer = FooterComponent::new(&session, &provider);
        let lines = footer.render_footer(80, &theme);
        let pwd_segment = if cfg!(windows) {
            "C:\\Users\\n\\proj".to_string()
        } else {
            "/pi-footer-oracle-cwd/proj".to_string()
        };
        assert_eq!(
            lines[0],
            format!("\x1b[38;2;126;136;142m{pwd_segment} (main) • my-session\x1b[39m")
        );
        assert_eq!(
            lines[1],
            "\x1b[38;2;126;136;142m↑1.6k ↓265 R100 W50 CH6.1% $0.560 (sub) 42.5%/200k (auto)\x1b[39m\x1b[38;2;126;136;142m         kimi-k2 • high\x1b[39m"
        );
        assert_eq!(lines[2], "first status second status");
    }

    #[test]
    fn footer_context_percent_colors_and_fallbacks() {
        let theme = dark();
        let provider = FixedProvider {
            branch: Some("main".to_string()),
            statuses: BTreeMap::new(),
            provider_count: 2,
        };
        let base = FixedSession {
            model: Some(FooterModel {
                id: "kimi-k2".to_string(),
                provider: "kimi-coding".to_string(),
                context_window: 100_000,
                reasoning: true,
            }),
            thinking_level: None,
            context_usage: Some((200_000, Some(91.1))),
            cwd: "C:\\Users\\n\\proj".to_string(),
            session_name: None,
            entries: Vec::new(),
            subscription_providers: Vec::new(),
            routed: None,
            entry_count: 0,
        };
        let over90 = FixedSession {
            thinking_level: base.thinking_level.clone(),
            context_usage: base.context_usage,
            cwd: base.cwd.clone(),
            session_name: base.session_name.clone(),
            entries: base.entries.clone(),
            subscription_providers: Vec::new(),
            model: base.model.clone(),
            routed: None,
            entry_count: 0,
        };
        let footer = FooterComponent::new(&over90, &provider);
        let lines = footer.render_footer(80, &theme);
        assert!(lines[1].contains("\x1b[38;2;234;127;129m91.1%/200k (auto)\x1b[39m"));

        let unknown = FixedSession {
            context_usage: Some((200_000, None)),
            ..base.clone()
        };
        let footer = FooterComponent::new(&unknown, &provider);
        assert!(footer.render_footer(80, &theme)[1].contains("?/200k (auto)"));

        let none = FixedSession {
            model: None,
            context_usage: None,
            ..base
        };
        let provider_one = FixedProvider {
            branch: None,
            statuses: BTreeMap::new(),
            provider_count: 1,
        };
        let footer = FooterComponent::new(&none, &provider_one);
        let lines = footer.render_footer(60, &theme);
        assert!(lines[1].contains("0.0%/0 (auto)"));
        // the dim wrapper's reset terminates the row (oracle noModel row 1)
        assert!(lines[1].ends_with("no-model\x1b[39m"));
    }

    #[test]
    fn sanitize_status_text_matches_upstream() {
        assert_eq!(sanitize_status_text("first\tstatus"), "first status");
        assert_eq!(sanitize_status_text("a\n\nb"), "a b");
        assert_eq!(sanitize_status_text("  x   y  "), "x y");
    }

    /// Delta oracle: `usage` entries count toward the totals (without
    /// touching the cache-hit rate) and a routed model appends
    /// `" → <physical id>[ • <level>]"` after the model/thinking segment.
    #[test]
    fn footer_usage_entries_and_routed_model() {
        let theme = dark();
        let provider = FixedProvider {
            branch: None,
            statuses: BTreeMap::new(),
            provider_count: 1,
        };
        let session = FixedSession {
            model: Some(FooterModel {
                id: "pi-virtual".to_string(),
                provider: "pi".to_string(),
                context_window: 100_000,
                reasoning: false,
            }),
            thinking_level: None,
            context_usage: Some((200_000, Some(10.0))),
            cwd: "C:\\Users\\n\\proj".to_string(),
            session_name: None,
            entries: vec![
                FooterEntry::Usage(usage(1500, 250, 100, 50, 0.5)),
                FooterEntry::Usage(usage(10, 5, 0, 0, 0.05)),
            ],
            subscription_providers: Vec::new(),
            routed: Some(FooterRoutedModel {
                model: FooterModel {
                    id: "claude-sonnet-4".to_string(),
                    provider: "anthropic".to_string(),
                    context_window: 200_000,
                    reasoning: true,
                },
                thinking_level: Some("high".to_string()),
            }),
            entry_count: 2,
        };
        let footer = FooterComponent::new(&session, &provider);
        let lines = footer.render_footer(120, &theme);
        // Totals include the usage entries (delta oracle footer_routed_and_usage:
        // the two entries sum to 1510/255 → "1.5k"/"255"); no CH part (usage
        // entries never set the cache-hit rate).
        assert!(
            lines[1].contains("↑1.5k ↓255 R100 W50 $0.550"),
            "{}",
            lines[1]
        );
        // The routed physical model renders where the latest response went.
        assert!(
            lines[1].contains("pi-virtual → claude-sonnet-4 • high"),
            "{}",
            lines[1]
        );

        // A routed model without a thinking level omits the level segment.
        let mut no_level = session.clone();
        no_level.routed = Some(FooterRoutedModel {
            thinking_level: None,
            ..no_level.routed.clone().unwrap()
        });
        let footer = FooterComponent::new(&no_level, &provider);
        // Delta oracle routed_without_level: the right-aligned row ends at
        // the model id (no trailing space) and omits the "• <level>" segment.
        let lines = footer.render_footer(120, &theme);
        let line = &lines[1];
        // Delta oracle routed_without_level: the right-aligned row ends at
        // the model id (then the dim reset) and omits "• <level>".
        assert!(line.contains("pi-virtual → claude-sonnet-4"), "{}", line);
        assert!(!line.contains("•"), "{}", line);
    }
}
