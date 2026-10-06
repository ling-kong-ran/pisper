//! Port of the deterministic core of upstream
//! `coding-agent/src/modes/interactive/bug-report.ts` (298 lines, new in the
//! delta): the `/bug` consent flow, its dialog strings, and the option
//! decisions.
//!
//! Seams (disclosed):
//! - The dialogs ride the shell's extension-dialog primitives
//!   ([`super::shell_lower`]: `show_extension_editor`,
//!   `extension_selector_choice`); upstream constructs the components
//!   directly.
//! - The bundle build/upload/export is the session surface
//!   ([`super::interactive_mode::ShellSession::build_bug_report_bundle`]);
//!   upstream calls `collectBugReportMetadata`/`writeBugReportArchive`/
//!   `uploadBugReport` inline. The metadata/diagnostics/zip cores live in
//!   `core/bug_report` (ported with their own oracles).
//! - The upload transport (`core/bug-report-upload.ts`, Radius gateway) has
//!   no vendored core; the flow models an upload failure with the upstream
//!   fallback choice.

/// Upstream `DISCLAIMER`.
pub const DISCLAIMER: &str = "This report goes to the Pi developers (Earendil) and is not shared publicly. It includes your pi version, operating system, the current model and provider configuration (without API keys), loaded extensions, settings, and provider error diagnostics from this session.";

/// Upstream `TRANSCRIPT_NOTE`.
pub const TRANSCRIPT_NOTE: &str = "The transcript contains your messages, model output, tool calls and their results, including file contents and command output read during this session.";

/// Upstream `BugReportDelivery`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BugReportDelivery {
    Upload,
    Zip,
}

impl BugReportDelivery {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Upload => "upload",
            Self::Zip => "zip",
        }
    }
}

/// Upstream `BugReportOptions`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BugReportOptions {
    pub hint: Option<String>,
    pub include_session: bool,
    pub include_summary: bool,
    pub delivery: BugReportDelivery,
}

/// The dialog answers `prompt_for_options` consumes (upstream awaits the
/// editor/selector dialogs; the port threads the answers as values).
#[derive(Debug, Clone, Default)]
pub struct PromptAnswers {
    /// The editor answer (`None` = cancelled).
    pub hint: Option<String>,
    /// The transcript chooser answer (`None` = dismissed).
    pub transcript: Option<String>,
    /// The summary-instead chooser answer (only consulted when the transcript
    /// is omitted; `None` = dismissed).
    pub summary: Option<String>,
    /// The delivery chooser answer (`None` = dismissed).
    pub delivery: Option<String>,
}

/// The dialog titles/labels the flow presents, in order (upstream literals).
pub const HINT_TITLE: &str = "Report a bug";
pub const HINT_PROMPT: &str = "What went wrong? (optional)";
pub const TRANSCRIPT_TITLE: &str = "Include the session transcript?";
pub const TRANSCRIPT_YES: &str = "Yes, include the transcript";
pub const TRANSCRIPT_NO: &str = "No";
pub const SUMMARY_YES_LABEL: &str = "Yes, generate a summary";
pub const SUMMARY_NO: &str = "No";
pub const DELIVERY_TITLE: &str = "Bug report";
pub const DELIVERY_UPLOAD: &str = "Upload Report";
pub const DELIVERY_ZIP: &str = "Export as Zip";
pub const DELIVERY_CANCEL: &str = "Cancel";
pub const FALLBACK_TITLE: &str = "Upload failed";
pub const OFFLINE_ERROR: &str =
    "Uploading bug reports requires online mode. Use Export as Zip instead.";
pub const CANCELLED_STATUS: &str = "Bug report cancelled";

/// Upstream `getRadiusGatewayUrl().host` (core/radius.ts): the
/// `PI_RADIUS_GATEWAY` override, else the vendored default gateway.
pub fn radius_gateway_host() -> String {
    let url =
        std::env::var("PI_RADIUS_GATEWAY").unwrap_or_else(|_| "https://radius.pi.dev".to_string());
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(&url);
    rest.trim_end_matches('/').to_string()
}

/// The transcript chooser's description
/// (upstream passes `TRANSCRIPT_NOTE` directly).
pub fn transcript_description() -> String {
    TRANSCRIPT_NOTE.to_string()
}

/// The summary chooser: title and description
/// (upstream `Attach a summary written by ${model?.name ?? "the current model"} instead?`).
pub fn summary_title(current_model_name: Option<&str>) -> String {
    format!(
        "Attach a summary written by {} instead?",
        current_model_name.unwrap_or("the current model")
    )
}

pub fn summary_description(current_model_provider: Option<&str>) -> String {
    format!(
        "The transcript is sent to {} with your credentials and tokens. Only the generated summary is attached; the transcript stays on your machine.",
        current_model_provider.unwrap_or("your provider")
    )
}

/// The delivery chooser description (upstream builds the confirmation body).
pub fn delivery_description(
    hint: &str,
    include_session: bool,
    include_summary: bool,
    current_model_name: Option<&str>,
    gateway_host: &str,
) -> String {
    let summary = if include_summary {
        format!(
            "written by {}",
            current_model_name.unwrap_or("the current model")
        )
    } else {
        "none".to_string()
    };
    format!(
        "Description: {}\nTranscript: {}\nSummary: {}\n\nUpload sends the report to {}. Export writes a zip archive to the current directory instead.",
        if hint.is_empty() { "none" } else { hint },
        if include_session { "included" } else { "not included" },
        summary,
        gateway_host,
    )
}

/// Upstream `promptForOptions`' decision core: the answers → options mapping
/// (the dialogs themselves are the shell seam). `None` = cancelled.
pub fn prompt_for_options(
    answers: &PromptAnswers,
    current_model_name: Option<&str>,
    current_model_provider: Option<&str>,
    gateway_host: &str,
) -> Option<BugReportOptions> {
    let hint = answers.hint.as_deref()?;
    let transcript = answers.transcript.as_deref()?;
    if transcript.is_empty() {
        return None;
    }
    let include_session = transcript != TRANSCRIPT_NO;
    let mut include_summary = false;
    if !include_session {
        let summary = answers.summary.as_deref()?;
        if summary.is_empty() {
            return None;
        }
        include_summary = summary != SUMMARY_YES_LABEL;
    }
    let description = hint.trim();
    let delivery = answers.delivery.as_deref()?;
    if delivery.is_empty() || delivery == DELIVERY_CANCEL {
        return None;
    }
    let _ = (current_model_name, current_model_provider, gateway_host);
    Some(BugReportOptions {
        hint: if description.is_empty() {
            None
        } else {
            Some(description.to_string())
        },
        include_session,
        include_summary,
        delivery: if delivery == DELIVERY_UPLOAD {
            BugReportDelivery::Upload
        } else {
            BugReportDelivery::Zip
        },
    })
}

/// Upstream `recordInSession`'s crash-log decision: the log clears only when
/// the report carried crashes.
pub fn clears_crash_log(diagnostics_crash_count: usize) -> bool {
    diagnostics_crash_count > 0
}
