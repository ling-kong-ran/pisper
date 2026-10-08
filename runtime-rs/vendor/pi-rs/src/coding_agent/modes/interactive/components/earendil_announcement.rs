//! Component glue mirrors upstream callback signatures whose types are
//! inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! Port of `modes/interactive/components/earendil-announcement.ts` (53 lines,
//! sha256
//! `49b1711624425b7d504bf7d871810225814138b64bb2152fe3e70dbf1dd005d4`).
//!
//! The bundled `clankolas.png` asset load is a filesystem seam: upstream
//! falls back to "no image" when the read fails; the port exposes the same
//! constructor with an optional base64 payload ([`Self::new_with_image`],
//! default `None` mirrors the vendored tree, which ships no asset).

use std::sync::Arc;

use crate::coding_agent::modes::interactive::components::model_selector::theme_fg;
use crate::coding_agent::modes::interactive::components::support::Border;
use crate::coding_agent::modes::interactive::theme::Theme;
use crate::tui::component::Component;
use crate::tui::component_mouse::ComponentHandle;
use crate::tui::components::layout_widgets::Spacer;
use crate::tui::components::text::Text;

const BLOG_URL: &str = "https://mariozechner.at/posts/2026-04-08-ive-sold-out/";
const IMAGE_FILENAME: &str = "clankolas.png";

/// Upstream `EarendilAnnouncementComponent`.
pub struct EarendilAnnouncementComponent {
    children: Vec<ComponentHandle>,
}

impl EarendilAnnouncementComponent {
    /// Upstream constructor with the vendored asset state (no bundled image).
    pub fn new(theme: Arc<Theme>) -> Self {
        Self::new_with_image(theme, None)
    }

    /// Upstream constructor with a successfully loaded image payload.
    // Upstream builds the child list incrementally (the image row is inserted
    // conditionally mid-sequence); keep the push-per-child shape.
    #[allow(clippy::vec_init_then_push)]
    pub fn new_with_image(theme: Arc<Theme>, image_base64: Option<&str>) -> Self {
        let accent_border = |theme: &Arc<Theme>| {
            let t = Arc::clone(theme);
            Border::new(Some(Box::new(move |s: &str| theme_fg(&t, "accent", s))))
        };
        let mut children: Vec<ComponentHandle> = Vec::new();
        children.push(ComponentHandle::new(accent_border(&theme)));
        children.push(ComponentHandle::new(Text::with_options(
            &theme.bold(&theme_fg(&theme, "accent", "pi has joined Earendil")),
            1,
            0,
            None,
        )));
        children.push(ComponentHandle::new(Spacer::new(1)));
        children.push(ComponentHandle::new(Text::with_options(
            &theme_fg(&theme, "muted", "Read the blog post:"),
            1,
            0,
            None,
        )));
        children.push(ComponentHandle::new(Text::with_options(
            &theme_fg(&theme, "mdLink", BLOG_URL),
            1,
            0,
            None,
        )));
        children.push(ComponentHandle::new(Spacer::new(1)));

        if let Some(image_base64) = image_base64 {
            // Upstream mounts an `Image` component (vendored
            // `tui::component_image`); the payload carries the same data.
            children.push(ComponentHandle::new(ImagePlaceholder {
                base64: image_base64.to_string(),
                mime_type: "image/png".to_string(),
                filename: IMAGE_FILENAME.to_string(),
                max_width_cells: 56,
            }));
            children.push(ComponentHandle::new(Spacer::new(1)));
        }

        children.push(ComponentHandle::new(accent_border(&theme)));
        Self { children }
    }
}

/// The image child payload (upstream `new Image(base64, "image/png", …,
/// { maxWidthCells: 56, filename })`).
struct ImagePlaceholder {
    // Carried for the shell's image seam; wired into the interactive shell
    // in r19+ (render surfaces only mime/filename, mirroring upstream's
    // fallback path).
    #[allow(dead_code)]
    base64: String,
    mime_type: String,
    filename: String,
    #[allow(dead_code)]
    max_width_cells: usize,
}

impl Component for ImagePlaceholder {
    fn render(&mut self, _width: usize) -> Vec<String> {
        vec![format!("[image {} {}]", self.mime_type, self.filename)]
    }
}

impl Component for EarendilAnnouncementComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        for child in &mut self.children {
            lines.extend(child.render(width));
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent::modes::interactive::theme::{load_builtin_theme, ColorMode};

    fn dark() -> Arc<Theme> {
        Arc::new(load_builtin_theme("dark", Some(ColorMode::Truecolor)).expect("dark"))
    }

    /// Oracle: tests/fixtures/interactive_r19_oracle scenario `earendil_announcement`
    /// — border/text/spacer sequence with the byte-exact themed strings (no
    /// bundled asset → no image child).
    #[test]
    fn earendil_announcement_matches_oracle() {
        let theme = dark();
        let mut component = EarendilAnnouncementComponent::new(theme.clone());
        assert_eq!(
            component.children.len(),
            7,
            "border/title/spacer/text/url/spacer/border"
        );
        let rendered = component.render(60);
        // oracle rows are the unpadded child texts; the rendered rows carry the
        // Text padding + width pad (trim_end) — render wide enough (the URL is
        // 54 columns padded) that nothing truncates.
        let expected = [
            format!("\x1b[38;2;167;152;215m{}\x1b[39m", "─".repeat(60)),
            " \x1b[1m\x1b[38;2;167;152;215mpi has joined Earendil\x1b[39m\x1b[22m".to_string(),
            " \x1b[38;2;157;165;169mRead the blog post:\x1b[39m".to_string(),
            " \x1b[38;2;105;173;208mhttps://mariozechner.at/posts/2026-04-08-ive-sold-out/\x1b[39m"
                .to_string(),
            format!("\x1b[38;2;167;152;215m{}\x1b[39m", "─".repeat(60)),
        ];
        let mut index = 0;
        for line in &rendered {
            if line.trim().is_empty() {
                continue;
            }
            assert_eq!(line.trim_end(), expected[index], "row {index}");
            index += 1;
        }
        assert_eq!(index, expected.len());
    }
}
