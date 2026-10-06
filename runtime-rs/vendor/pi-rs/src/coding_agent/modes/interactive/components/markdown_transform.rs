//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/components/markdown-transform.ts` (29 lines,
//! sha256 `657edd0453dab59d362e0d18c330486b86a47fe7a33b8c32204db8ce23a142a9`):
//! the extension transformer pipeline handed to the Markdown widget as its
//! `transform` option (a `(markdown, availableWidth) -> string` callback).

use std::sync::Arc;

use crate::tui::components::markdown::TransformFn;

/// Upstream `MarkdownTransformContext`
/// (`core/extensions/types.ts`): `{ messageType, isStreaming, availableWidth }`.
#[derive(Clone, Debug)]
pub struct MarkdownTransformContext {
    pub message_type: String,
    pub is_streaming: bool,
    pub available_width: usize,
}

/// Upstream `MarkdownTransformer`: `(markdown, context) => string | undefined`.
/// Returning `None` keeps the current Markdown (upstream returns a non-string).
/// A panicking transformer mirrors an upstream `throw` (also keeps current).
pub type MarkdownTransformer =
    Box<dyn Fn(&str, &MarkdownTransformContext) -> Option<String> + Send + Sync>;

/// Upstream `applyMarkdownTransformers`: run every transformer in order,
/// keeping the current Markdown when a transformer throws or returns a
/// non-string.
pub fn apply_markdown_transformers(
    markdown: &str,
    context: &MarkdownTransformContext,
    transformers: &[MarkdownTransformer],
) -> String {
    let mut transformed = markdown.to_string();
    for transformer in transformers {
        let next = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            transformer(&transformed, context)
        }))
        .ok()
        .flatten();
        if let Some(next) = next {
            transformed = next;
        }
    }
    transformed
}

/// Upstream `createMarkdownTransform`: build the Markdown widget's
/// `transform: (markdown, availableWidth) => string` callback.
pub fn create_markdown_transform(
    message_type: &str,
    is_streaming: bool,
    transformers: Arc<Vec<MarkdownTransformer>>,
) -> TransformFn {
    let message_type = message_type.to_string();
    Arc::new(move |markdown: &str, available_width: usize| {
        let context = MarkdownTransformContext {
            message_type: message_type.clone(),
            is_streaming,
            available_width,
        };
        apply_markdown_transformers(markdown, &context, &transformers)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oracle_pipeline() -> Vec<MarkdownTransformer> {
        vec![
            Box::new(|md: &str, ctx: &MarkdownTransformContext| {
                if md.contains("SWAP") {
                    Some(md.replace("SWAP", &format!("swapped:{}", ctx.message_type)))
                } else {
                    None
                }
            }),
            Box::new(|_md: &str, _ctx: &MarkdownTransformContext| {
                panic!("boom"); // upstream `throw` → keep current Markdown
            }),
            Box::new(|_md: &str, _ctx: &MarkdownTransformContext| None), // non-string → keep
            Box::new(|md: &str, _ctx: &MarkdownTransformContext| Some(format!("{md}|t4"))),
        ]
    }

    /// Oracle: tests/fixtures/interactive_r19_oracle scenario `markdown_transform` —
    /// exact strings from the verbatim upstream module.
    #[test]
    fn transformer_pipeline_matches_oracle() {
        let transformers = Arc::new(oracle_pipeline());
        let t = create_markdown_transform("user", false, transformers);
        assert_eq!(t("hello", 40), "hello|t4");
        assert_eq!(t("SWAP text", 40), "swapped:user text|t4");
        assert_eq!(t("", 0), "|t4");

        let plain = create_markdown_transform("assistant", true, Arc::new(Vec::new()));
        assert_eq!(
            plain(&format!("x{}", "y".repeat(50)), 10),
            format!("x{}", "y".repeat(50))
        );

        let thinking =
            create_markdown_transform("assistant-thinking", false, Arc::new(oracle_pipeline()));
        assert_eq!(thinking("SWAP", 5), "swapped:assistant-thinking|t4");
    }
}
