//! Tests for the scroll-view and settings-list component ports
//! (upstream components/scroll-view.ts + components/settings-list.ts).

use std::sync::{Arc, Mutex};

use crate::tui::component::Component;
use crate::tui::components::scroll_view::{
    ScrollView, ScrollViewOptions, ScrollViewScrollToOptions, ScrollViewScrollbar,
};
use crate::tui::components::settings_list::{
    SettingItem, SettingsList, SettingsListOptions, SettingsListTheme,
};
use crate::tui::components::text::Text;

fn scroll_view(follow_end: bool) -> ScrollView {
    ScrollView::new(
        Box::new(Text::with_options("", 0, 0, None)),
        ScrollViewOptions {
            follow_end,
            ..Default::default()
        },
    )
}

// ---------------------------------------------------------------------------
// ScrollView
// ---------------------------------------------------------------------------

#[test]
fn scroll_view_follow_end_pins_to_max_scroll() {
    let mut view = scroll_view(true);
    view.update_layout(50, 10);
    assert_eq!(view.scroll_top(), 40);
    assert!(view.is_following_end());

    // Growing content keeps the view pinned to the end.
    view.update_layout(60, 10);
    assert_eq!(view.scroll_top(), 50);
    assert!(view.is_following_end());
}

#[test]
fn scroll_view_clamps_scroll_top_to_content() {
    let mut view = scroll_view(false);
    view.update_layout(30, 10);
    view.scroll_to(100, ScrollViewScrollToOptions::default());
    assert_eq!(view.scroll_top(), 20);
    view.scroll_to(0, ScrollViewScrollToOptions::default());
    assert_eq!(view.scroll_top(), 0);
}

#[test]
fn scroll_view_scroll_by_returns_unconsumed_remainder() {
    let mut view = scroll_view(false);
    view.update_layout(30, 10);
    // Overscroll down by 25 with only 20 available.
    let remainder = view.scroll_by(25);
    assert_eq!(remainder, 5);
    assert_eq!(view.scroll_top(), 20);
    // Overscroll up by 25 with 20 available.
    let remainder = view.scroll_by(-25);
    assert_eq!(remainder, -5);
    assert_eq!(view.scroll_top(), 0);
}

#[test]
fn scroll_view_disable_follow_suppresses_follow_at_end() {
    let mut view = scroll_view(true);
    view.update_layout(30, 10);
    assert!(view.is_following_end());

    view.scroll_to(
        20,
        ScrollViewScrollToOptions {
            disable_follow: true,
        },
    );
    assert!(!view.is_following_end());

    // Non-suppressed scroll to the same position re-enables follow.
    view.scroll_to(20, ScrollViewScrollToOptions::default());
    assert!(view.is_following_end());
}

#[test]
fn scroll_view_content_width_reserves_scrollbar_column() {
    let mut view = scroll_view(false);
    view.set_scrollbar(ScrollViewScrollbar::Always);
    assert_eq!(view.content_width(80), 79);

    let mut view = scroll_view(false);
    view.set_scrollbar(ScrollViewScrollbar::Hidden);
    assert_eq!(view.content_width(80), 80);
}

#[test]
fn scroll_view_render_pads_lines_when_scrollbar_reserved() {
    let mut view = scroll_view(false);
    view.set_scrollbar(ScrollViewScrollbar::Always);
    view.update_layout(2, 10);
    let lines = view.render(20);
    for line in &lines {
        assert!(
            line.ends_with(' '),
            "line should carry the scrollbar pad: {line:?}"
        );
    }
}

#[test]
fn scroll_view_child_renders_through_the_wrapper() {
    struct PassThrough;
    impl Component for PassThrough {
        fn render(&mut self, _width: usize) -> Vec<String> {
            vec!["content line".to_string()]
        }
    }

    let mut view = ScrollView::new(Box::new(PassThrough), ScrollViewOptions::default());
    view.update_layout(1, 10);
    assert_eq!(view.render(40), vec!["content line".to_string()]);
}

// ---------------------------------------------------------------------------
// SettingsList
// ---------------------------------------------------------------------------

fn settings_items() -> Vec<SettingItem> {
    vec![
        SettingItem {
            id: "theme".into(),
            label: "Theme".into(),
            description: Some("Color theme".into()),
            current_value: "dark".into(),
            values: vec!["dark".into(), "light".into()],
        },
        SettingItem {
            id: "model".into(),
            label: "Model".into(),
            description: None,
            current_value: "gpt".into(),
            values: vec!["gpt".into(), "claude".into()],
        },
    ]
}

fn default_theme() -> SettingsListTheme {
    SettingsListTheme::default()
}

#[test]
fn settings_list_selection_wraps_and_values_cycle() {
    let changes: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let changes_sink = Arc::clone(&changes);
    let mut list = SettingsList::new(
        settings_items(),
        5,
        default_theme(),
        Box::new(move |id, value| {
            changes_sink
                .lock()
                .unwrap()
                .push((id.to_string(), value.to_string()))
        }),
        Box::new(|| {}),
        SettingsListOptions::default(),
    );

    // Down wraps from last to first and vice versa.
    list.handle_input("\x1b[B"); // down: index 0 -> 1
    list.handle_input("\x1b[B"); // down: wraps to 0
    list.handle_input("\x1b[A"); // up: wraps to last (model)

    // Space cycles the model value.
    list.handle_input(" ");
    assert_eq!(
        list.get_display_item_value("model"),
        Some("claude".to_string())
    );
    assert_eq!(
        changes.lock().unwrap().last(),
        Some(&("model".to_string(), "claude".to_string()))
    );
}

#[test]
fn settings_list_cancel_invokes_callback() {
    let mut list = SettingsList::new(
        settings_items(),
        5,
        default_theme(),
        Box::new(|_, _| {}),
        Box::new(|| {}),
        SettingsListOptions::default(),
    );
    list.handle_input("\x1b"); // Escape
}

#[test]
fn settings_list_update_value_and_select_item() {
    let mut list = SettingsList::new(
        settings_items(),
        5,
        default_theme(),
        Box::new(|_, _| {}),
        Box::new(|| {}),
        SettingsListOptions::default(),
    );
    list.update_value("theme", "light");
    list.select_item("theme");
    assert_eq!(
        list.get_display_item_value("theme"),
        Some("light".to_string())
    );
}

#[test]
fn settings_list_render_shows_labels_values_and_description() {
    let mut list = SettingsList::new(
        settings_items(),
        5,
        default_theme(),
        Box::new(|_, _| {}),
        Box::new(|| {}),
        SettingsListOptions::default(),
    );
    let lines = list.render(80);
    let joined = lines.join("\n");
    assert!(joined.contains("Theme"));
    assert!(joined.contains("dark"));
    assert!(joined.contains("Color theme"));
    // Hint line is present.
    assert!(joined.contains("Enter/Space to change"));
}

// ---------------------------------------------------------------------------
// Box / Spacer / TruncatedText
// ---------------------------------------------------------------------------

#[test]
fn box_pads_children_and_applies_background() {
    let mut b = crate::tui::components::layout_widgets::Box::new();
    b.set_padding(1, 1);
    b.add_child(Box::new(crate::tui::components::text::Text::with_options(
        "hi", 0, 0, None,
    )));
    let lines = b.render(10);
    assert_eq!(lines.len(), 3);
    assert!(lines[0].chars().all(|c| c == ' '));
    assert!(lines[1].starts_with(" hi "));
}

#[test]
fn box_clear_removes_children() {
    let mut b = crate::tui::components::layout_widgets::Box::new();
    b.add_child(Box::new(crate::tui::components::text::Text::with_options(
        "hi", 0, 0, None,
    )));
    b.clear();
    assert_eq!(b.render(10), Vec::<String>::new());
}

#[test]
fn spacer_renders_empty_lines() {
    let mut spacer = crate::tui::components::layout_widgets::Spacer::new(3);
    assert_eq!(spacer.render(20), vec!["".to_string(); 3]);
}

#[test]
fn truncated_text_stops_at_newline_and_truncates() {
    let mut widget =
        crate::tui::components::layout_widgets::TruncatedText::with_padding("first\nsecond", 0, 0);
    let lines = widget.render(10);
    assert_eq!(lines, vec!["first".to_string() + &" ".repeat(5)]);

    let mut widget = crate::tui::components::layout_widgets::TruncatedText::new("01234567890123");
    let lines = widget.render(5);
    assert_eq!(lines.len(), 1);
    assert_eq!(crate::tui::utils::visible_width(&lines[0]), 5);
}
