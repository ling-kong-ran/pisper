//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/components/mermaid.ts` (89 lines, sha256
//! `994acb562b70957ed828770d74879e03cfffbc472acef94073ad74cda8defe58`).
//!
//! Deterministic parse face ported in full: the `isMermaid` token predicate
//! ([`is_mermaid`]) and the `codeSpan` encoder ([`code_span`]) — the CommonMark
//! backtick-fence selection, padding and the NBSP blank-row substitution —
//! plus the gating logic of `createMermaidMarkdownTransformer`
//! ([`MermaidRenderingMode`]/[`mermaid_transform_gate`]).
//!
//! Disclosed seam (S19.8 in `components/mod.rs`): the diagram renderer itself
//! (`grok-mermaid` `render()` → `MermaidArt`) and the upstream `Marked` lexer
//! are not part of the vendored tree, so the transformer's token walk is
//! exposed as [`MermaidRenderingPlan`] — the caller supplies per-code-block
//! art resolution, and this module owns every decision around it (mode gating,
//! warning suffixes, row encoding).

/// Upstream `MermaidRenderingMode` (`core/settings-manager.ts`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MermaidRenderingMode {
    Off,
    #[default]
    Auto,
    Streaming,
}

/// Upstream `isMermaid`: a `code` token whose first language word (before any
/// whitespace, lowercased) is `mermaid`.
pub fn is_mermaid(token_type: &str, lang: Option<&str>) -> bool {
    token_type == "code"
        && lang
            .and_then(|lang| lang.split_whitespace().next())
            .is_some_and(|first| first.to_lowercase() == "mermaid")
}

/// Upstream `codeSpan`: encode a diagram row as an inline code span so
/// Markdown preserves its spacing and box-drawing characters.
pub fn code_span(line: &str) -> String {
    // NBSP for blank rows: an empty code span has no visible height.
    let content = if line.is_empty() { "\u{a0}" } else { line };
    // CommonMark code spans use matching backtick delimiters, so choose one
    // longer than any backtick run in the content.
    let longest_backtick_run = content
        .match_indices('`')
        .map(|(start, _)| content[start..].chars().take_while(|c| *c == '`').count())
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest_backtick_run + 1);
    let padding = if content.starts_with('`') || content.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{fence}{padding}{content}{padding}{fence}")
}

/// Upstream `themedLines` span classes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpanClass {
    Border,
    Text,
    Edge,
    EdgeLabel,
    Title,
    None,
}

/// Upstream `styleSpan`.
pub fn style_span(
    theme: &crate::coding_agent::modes::interactive::theme::Theme,
    class: SpanClass,
    text: &str,
) -> String {
    use crate::coding_agent::modes::interactive::components::model_selector::theme_fg;
    match class {
        SpanClass::Border => theme_fg(theme, "borderMuted", text),
        SpanClass::Text => theme_fg(theme, "text", text),
        SpanClass::Edge => theme_fg(theme, "accent", text),
        SpanClass::EdgeLabel => theme_fg(theme, "muted", text),
        SpanClass::Title => theme_fg(theme, "accent", &theme.bold(text)),
        SpanClass::None => text.to_string(),
    }
}

/// The decision the transformer makes for one Mermaid code block
/// (upstream's inline branch chain).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MermaidRenderingPlan {
    /// Not a Mermaid block → keep `token.raw`.
    KeepRaw,
    /// Mode off / thinking context / streaming with non-streaming mode → raw.
    SkipByGate,
    /// Diagram wider than `availableWidth` → raw.
    TooWide {
        width: usize,
        available_width: usize,
    },
    /// Non-streaming with warnings → raw + styled warning line.
    Warnings { warnings: Vec<String> },
    /// Render the styled rows (hard-break joined) — `themedLines` output.
    Render { lines: Vec<String> },
}

/// Resolve one Mermaid block's fate (upstream `createMermaidMarkdownTransformer`
/// body; `art` carries the would-be `MermaidArt.width/warnings/styled/plain`).
pub fn mermaid_transform_gate(
    mode: MermaidRenderingMode,
    message_type: &str,
    is_streaming: bool,
    available_width: usize,
    has_theme: bool,
    art: Option<MermaidArtProbe>,
) -> MermaidRenderingPlan {
    if mode == MermaidRenderingMode::Off
        || message_type == "assistant-thinking"
        || (is_streaming && mode != MermaidRenderingMode::Streaming)
    {
        return MermaidRenderingPlan::SkipByGate;
    }
    let Some(art) = art else {
        return MermaidRenderingPlan::KeepRaw;
    };
    if art.width > available_width {
        return MermaidRenderingPlan::TooWide {
            width: art.width,
            available_width,
        };
    }
    if !is_streaming && !art.warnings.is_empty() {
        return MermaidRenderingPlan::Warnings {
            warnings: art.warnings,
        };
    }
    if has_theme {
        MermaidRenderingPlan::Render { lines: art.styled }
    } else {
        MermaidRenderingPlan::Render { lines: art.plain }
    }
}

/// The `MermaidArt` surface the transformer reads (renderer seam S19.8).
#[derive(Clone, Debug, Default)]
pub struct MermaidArtProbe {
    pub width: usize,
    pub warnings: Vec<String>,
    /// `styled` rows: spans joined (already styled by the caller).
    pub styled: Vec<String>,
    /// `plain` rows.
    pub plain: Vec<String>,
}

/// Upstream warning suffix assembly (`(+N more)`).
pub fn mermaid_warning_prefix(warnings: &[String]) -> Option<String> {
    let first = warnings.first()?;
    let suffix = if warnings.len() > 1 {
        format!(" (+{})", warnings.len() - 1)
    } else {
        String::new()
    };
    Some(format!("Mermaid diagram not rendered: {first}{suffix}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Oracle: tests/fixtures/interactive_r19_oracle scenario `mermaid_code_span` —
    /// byte-exact fences/padding and the token predicate truth table.
    #[test]
    fn code_span_and_predicate_match_oracle() {
        assert_eq!(code_span("┌──┐"), "`┌──┐`");
        assert_eq!(code_span(""), "`\u{a0}`");
        // padding only when the content starts/ends with a backtick (oracle rows)
        assert_eq!(code_span("plain ` tick"), "``plain ` tick``");
        assert_eq!(code_span("two `` ticks"), "```two `` ticks```");
        assert_eq!(code_span("`edge`"), "`` `edge` ``");
        assert_eq!(code_span("``"), "``` `` ```");

        assert!(is_mermaid("code", Some("mermaid")));
        assert!(is_mermaid("code", Some(" MERMAID ")));
        assert!(is_mermaid("code", Some("mermaid extra")));
        assert!(!is_mermaid("code", Some("mermaidx")));
        assert!(!is_mermaid("code", None));
        assert!(!is_mermaid("text", Some("mermaid")));
        assert!(!is_mermaid("code", Some("  ")));
    }

    #[test]
    fn gate_matches_upstream_branches() {
        let art = MermaidArtProbe {
            width: 40,
            warnings: vec!["bad syntax".to_string()],
            styled: vec!["row".to_string()],
            plain: vec!["row".to_string()],
        };
        // mode off / thinking / streaming gates
        assert_eq!(
            mermaid_transform_gate(
                MermaidRenderingMode::Off,
                "user",
                false,
                80,
                true,
                Some(art.clone())
            ),
            MermaidRenderingPlan::SkipByGate
        );
        assert_eq!(
            mermaid_transform_gate(
                MermaidRenderingMode::Auto,
                "assistant-thinking",
                false,
                80,
                true,
                Some(art.clone())
            ),
            MermaidRenderingPlan::SkipByGate
        );
        assert_eq!(
            mermaid_transform_gate(
                MermaidRenderingMode::Auto,
                "user",
                true,
                80,
                true,
                Some(art.clone())
            ),
            MermaidRenderingPlan::SkipByGate
        );
        assert_eq!(
            mermaid_transform_gate(
                MermaidRenderingMode::Streaming,
                "user",
                true,
                80,
                true,
                Some(art.clone())
            ),
            MermaidRenderingPlan::Render {
                lines: vec!["row".to_string()]
            }
        );
        // warnings short-circuit only when not streaming
        assert_eq!(
            mermaid_transform_gate(
                MermaidRenderingMode::Auto,
                "user",
                false,
                80,
                true,
                Some(art.clone())
            ),
            MermaidRenderingPlan::Warnings {
                warnings: vec!["bad syntax".to_string()]
            }
        );
        // too wide
        assert_eq!(
            mermaid_transform_gate(
                MermaidRenderingMode::Auto,
                "user",
                false,
                20,
                true,
                Some(art.clone())
            ),
            MermaidRenderingPlan::TooWide {
                width: 40,
                available_width: 20
            }
        );
        // no warnings → renders themed/plain rows
        let clean_art = MermaidArtProbe {
            width: 40,
            warnings: Vec::new(),
            styled: vec!["row".to_string()],
            plain: vec!["row".to_string()],
        };
        assert_eq!(
            mermaid_transform_gate(
                MermaidRenderingMode::Auto,
                "user",
                false,
                80,
                true,
                Some(clean_art.clone())
            ),
            MermaidRenderingPlan::Render {
                lines: vec!["row".to_string()]
            }
        );
        assert_eq!(
            mermaid_transform_gate(
                MermaidRenderingMode::Auto,
                "user",
                false,
                80,
                false,
                Some(clean_art)
            ),
            MermaidRenderingPlan::Render {
                lines: vec!["row".to_string()]
            }
        );
        // warnings rendered as the styled warning suffix
        assert_eq!(
            mermaid_warning_prefix(&["bad syntax".to_string()]),
            Some("Mermaid diagram not rendered: bad syntax".to_string())
        );
        assert_eq!(
            mermaid_warning_prefix(&["a".to_string(), "b".to_string(), "c".to_string()]),
            Some("Mermaid diagram not rendered: a (+2)".to_string())
        );
        assert_eq!(mermaid_warning_prefix(&[]), None);
    }

    #[test]
    fn span_classes_use_theme_colors() {
        let theme = Arc::new(
            crate::coding_agent::modes::interactive::theme::load_builtin_theme(
                "dark",
                Some(crate::coding_agent::modes::interactive::theme::ColorMode::Truecolor),
            )
            .expect("dark"),
        );
        // borderMuted → darkGray → #505050 (dark.json vars)
        assert_eq!(
            style_span(&theme, SpanClass::Border, "x"),
            "\x1b[38;2;118;129;134mx\x1b[39m"
        );
        assert!(style_span(&theme, SpanClass::Text, "x").starts_with("\x1b[38;2;222;224;225m"));
        assert!(style_span(&theme, SpanClass::Edge, "x").starts_with("\x1b[38;2;167;152;215m"));
        assert!(style_span(&theme, SpanClass::EdgeLabel, "x").starts_with("\x1b[38;2;157;165;169m"));
        assert!(
            style_span(&theme, SpanClass::Title, "x").starts_with("\x1b[38;2;167;152;215m\x1b[1m")
        );
        assert_eq!(style_span(&theme, SpanClass::None, "x"), "x");
    }
}
