//! Vertical ScrollView, including live layout state, follow suppression,
//! render callbacks and cancellable transient scrollbar timers.
//!
//! Layout frames hold a clonable ScrollHandle rather than a stale state copy.
//! Callbacks/styles are invoked outside locks. The default scheduler uses an
//! unjoined sleeping thread (like an unref'ed Node timer); generation tokens
//! cancel old timers and weak ownership prevents callbacks after state drop.
//! Tests/hosts may supply a deterministic event-loop scheduler instead.
use crate::tui::component::{Component, TuiMouseEvent};
use crate::tui::component_mouse::{
    container_mouse_action, ensure_handle, valid_container_row, MouseAction,
};
use crate::tui::layout_node::LayoutNode;
use crate::tui::rendered_lines::RenderedLines;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollViewScrollbar {
    Hidden,
    Auto,
    Always,
}
pub type ScrollStyle = Arc<dyn Fn(&str) -> String + Send + Sync>;
pub type RequestRender = Arc<dyn Fn() + Send + Sync>;
pub type ScrollTimerCallback = Box<dyn FnOnce() + Send>;
pub type ScrollTimerScheduler = Arc<dyn Fn(Duration, ScrollTimerCallback) + Send + Sync>;

#[derive(Clone, Default)]
pub struct ScrollViewOptions {
    pub follow_end: bool,
    pub primary: bool,
    pub overscroll_contain: bool,
    pub scrollbar: Option<ScrollViewScrollbar>,
    pub scrollbar_track_style: Option<ScrollStyle>,
    pub scrollbar_thumb_style: Option<ScrollStyle>,
    pub scrollbar_hide_delay_ms: Option<f64>,
    pub timer_scheduler: Option<ScrollTimerScheduler>,
}
impl std::fmt::Debug for ScrollViewOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScrollViewOptions")
            .field("follow_end", &self.follow_end)
            .field("primary", &self.primary)
            .field("overscroll_contain", &self.overscroll_contain)
            .field("scrollbar", &self.scrollbar)
            .field("scrollbar_hide_delay_ms", &self.scrollbar_hide_delay_ms)
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct ScrollViewScrollToOptions {
    pub disable_follow: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct ScrollSnapshot {
    pub scroll_top: usize,
    pub content_height: usize,
    pub viewport_height: usize,
    pub following_end: bool,
    pub scrollbar: ScrollViewScrollbar,
    pub scrollbar_visible: bool,
    pub scrollbar_active: bool,
    pub primary: bool,
    pub overscroll_contain: bool,
}
struct ScrollState {
    follow_end: bool,
    primary: bool,
    overscroll_contain: bool,
    scrollbar: ScrollViewScrollbar,
    scroll_top: usize,
    content_height: usize,
    viewport_height: usize,
    following_end: bool,
    follow_suppressed_at_end: bool,
    transient_visible: bool,
    active: bool,
    track_style: ScrollStyle,
    thumb_style: ScrollStyle,
    hide_delay: Duration,
    timer_generation: u64,
    request_render: Option<RequestRender>,
}
impl ScrollState {
    fn max_top(&self) -> usize {
        self.content_height.saturating_sub(self.viewport_height)
    }
    fn hide_transient(&mut self) {
        self.transient_visible = false;
        self.timer_generation += 1;
    }
    fn mark_activity(&mut self) -> Option<(u64, Duration)> {
        if self.scrollbar != ScrollViewScrollbar::Auto
            || self.content_height <= self.viewport_height
        {
            return None;
        }
        self.transient_visible = true;
        self.timer_generation += 1;
        (!self.active).then_some((self.timer_generation, self.hide_delay))
    }
    fn update_layout(&mut self, content: usize, viewport: usize) {
        self.content_height = content;
        self.viewport_height = viewport;
        let max = self.max_top();
        self.scroll_top = if self.following_end {
            max
        } else {
            self.scroll_top.min(max)
        };
        if self.scroll_top < max {
            self.follow_suppressed_at_end = false;
        }
        if self.follow_end && self.scroll_top == max && !self.follow_suppressed_at_end {
            self.following_end = true;
        }
        if content <= viewport {
            self.hide_transient();
        }
    }
}
#[derive(Default)]
struct Effects {
    notify: bool,
    timer: Option<(u64, Duration)>,
}

/// Shared scroll identity; equality is state identity, not matching values.
#[derive(Clone)]
pub struct ScrollHandle {
    state: Arc<Mutex<ScrollState>>,
    scheduler: ScrollTimerScheduler,
}
impl PartialEq for ScrollHandle {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.state, &other.state)
    }
}
impl Eq for ScrollHandle {}
impl std::fmt::Debug for ScrollHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.snapshot().fmt(f)
    }
}
impl ScrollHandle {
    fn new(options: ScrollViewOptions) -> Self {
        let delay = options.scrollbar_hide_delay_ms.unwrap_or(1000.0).floor();
        // Node normalizes zero/negative/nonfinite/overflowing setTimeout delays
        // to 1ms. The upstream constructor floors/clamps before scheduling.
        let millis = if !delay.is_finite() || !(1.0..=2_147_483_647.0).contains(&delay) {
            1
        } else {
            delay as u64
        };
        Self {
            state: Arc::new(Mutex::new(ScrollState {
                follow_end: options.follow_end,
                primary: options.primary,
                overscroll_contain: options.overscroll_contain,
                scrollbar: options.scrollbar.unwrap_or(ScrollViewScrollbar::Hidden),
                scroll_top: 0,
                content_height: 0,
                viewport_height: 0,
                following_end: options.follow_end,
                follow_suppressed_at_end: false,
                transient_visible: false,
                active: false,
                track_style: options
                    .scrollbar_track_style
                    .unwrap_or_else(|| Arc::new(|s| format!("\x1b[90m{s}\x1b[39m"))),
                thumb_style: options
                    .scrollbar_thumb_style
                    .unwrap_or_else(|| Arc::new(|s| format!("\x1b[37m{s}\x1b[39m"))),
                hide_delay: Duration::from_millis(millis),
                timer_generation: 0,
                request_render: None,
            })),
            scheduler: options.timer_scheduler.unwrap_or_else(|| {
                Arc::new(|delay, callback| {
                    std::thread::spawn(move || {
                        std::thread::sleep(delay);
                        callback();
                    });
                })
            }),
        }
    }
    fn mutate<R>(&self, action: impl FnOnce(&mut ScrollState, &mut Effects) -> R) -> R {
        let (result, effects, callback) = {
            let mut state = self.state.lock().unwrap();
            let mut effects = Effects::default();
            let result = action(&mut state, &mut effects);
            let callback = effects
                .notify
                .then(|| state.request_render.clone())
                .flatten();
            (result, effects, callback)
        };
        if let Some((generation, delay)) = effects.timer {
            let weak = Arc::downgrade(&self.state);
            (self.scheduler)(
                delay,
                Box::new(move || {
                    let Some(state) = weak.upgrade() else { return };
                    let callback = {
                        let mut state = state.lock().unwrap();
                        if state.timer_generation != generation {
                            return;
                        }
                        state.timer_generation += 1;
                        state.transient_visible = false;
                        state.request_render.clone()
                    };
                    if let Some(callback) = callback {
                        callback();
                    }
                }),
            );
        }
        if let Some(callback) = callback {
            callback();
        }
        result
    }
    /// Configured follow policy, distinct from the current following state.
    pub fn follow_end(&self) -> bool {
        self.state.lock().unwrap().follow_end
    }
    pub fn snapshot(&self) -> ScrollSnapshot {
        let s = self.state.lock().unwrap();
        ScrollSnapshot {
            scroll_top: s.scroll_top,
            content_height: s.content_height,
            viewport_height: s.viewport_height,
            following_end: s.following_end,
            scrollbar: s.scrollbar,
            scrollbar_visible: if s.scrollbar == ScrollViewScrollbar::Always {
                s.viewport_height > 0
            } else {
                s.scrollbar == ScrollViewScrollbar::Auto
                    && s.content_height > s.viewport_height
                    && s.transient_visible
            },
            scrollbar_active: s.active,
            primary: s.primary,
            overscroll_contain: s.overscroll_contain,
        }
    }
    pub fn content_width(&self, width: usize) -> usize {
        if self.snapshot().scrollbar == ScrollViewScrollbar::Always && width > 1 {
            width - 1
        } else {
            width
        }
    }
    pub fn update_layout(
        &self,
        content_height: usize,
        viewport_height: usize,
        request_render: RequestRender,
    ) {
        self.mutate(|s, _| {
            s.request_render = Some(request_render);
            s.update_layout(content_height, viewport_height);
        });
    }
    pub fn set_scrollbar(&self, scrollbar: ScrollViewScrollbar) {
        self.mutate(|s, e| {
            if s.scrollbar == scrollbar {
                return;
            }
            s.scrollbar = scrollbar;
            if scrollbar != ScrollViewScrollbar::Auto {
                s.hide_transient();
            } else if s.active {
                e.timer = s.mark_activity();
            }
            e.notify = true;
        });
    }
    pub fn set_scrollbar_active(&self, active: bool) {
        self.mutate(|s, e| {
            if s.active == active {
                return;
            }
            s.active = active;
            e.timer = s.mark_activity();
            e.notify = true;
        });
    }
    pub fn scroll_to_number(&self, scroll_top: f64, options: ScrollViewScrollToOptions) {
        self.mutate(|s, e| {
            let requested = if scroll_top.is_finite() {
                scroll_top.trunc()
            } else {
                s.scroll_top as f64
            };
            let max = s.max_top();
            let next = requested.clamp(0.0, max as f64) as usize;
            let suppressed = options.disable_follow && next == max;
            let following = !suppressed && s.follow_end && next == max;
            if next == s.scroll_top
                && following == s.following_end
                && suppressed == s.follow_suppressed_at_end
            {
                return;
            }
            let moved = next != s.scroll_top;
            s.scroll_top = next;
            s.following_end = following;
            s.follow_suppressed_at_end = suppressed;
            if moved {
                e.timer = s.mark_activity();
            }
            e.notify = true;
        });
    }
    pub fn scroll_to(&self, top: usize, options: ScrollViewScrollToOptions) {
        self.scroll_to_number(top as f64, options);
    }
    pub fn scroll_by_number(&self, lines: f64) -> f64 {
        self.mutate(|s, e| {
            let requested = if lines.is_finite() {
                lines.trunc()
            } else {
                0.0
            };
            if requested == 0.0 {
                return 0.0;
            }
            let max = s.max_top();
            let start = if s.following_end { max } else { s.scroll_top };
            let next = (start as f64 + requested).clamp(0.0, max as f64) as usize;
            let moved = next as f64 - start as f64;
            let was_following = s.following_end;
            s.scroll_top = next;
            s.following_end = s.follow_end && next == max;
            s.follow_suppressed_at_end = false;
            if moved != 0.0 {
                e.timer = s.mark_activity();
            }
            e.notify = moved != 0.0 || s.following_end != was_following;
            requested - moved
        })
    }
    pub fn scroll_by(&self, lines: i64) -> i64 {
        self.scroll_by_number(lines as f64) as i64
    }
    pub fn scroll_to_start(&self) {
        self.mutate(|s, e| {
            let following = s.follow_end && s.content_height <= s.viewport_height;
            let changed = s.scroll_top != 0 || s.following_end != following;
            s.scroll_top = 0;
            s.following_end = following;
            s.follow_suppressed_at_end = false;
            if changed {
                e.timer = s.mark_activity();
                e.notify = true;
            }
        });
    }
    pub fn scroll_to_end(&self) {
        self.mutate(|s, e| {
            let next = s.max_top();
            let changed = s.scroll_top != next || s.following_end != s.follow_end;
            s.scroll_top = next;
            s.following_end = s.follow_end;
            s.follow_suppressed_at_end = false;
            if changed {
                e.timer = s.mark_activity();
                e.notify = true;
            }
        });
    }
    pub fn style_scrollbar(&self, thumb: bool, text: &str) -> String {
        let style = {
            let s = self.state.lock().unwrap();
            if thumb {
                s.thumb_style.clone()
            } else {
                s.track_style.clone()
            }
        };
        style(text)
    }
}

pub struct ScrollView {
    child: Box<dyn Component>,
    state: ScrollHandle,
}
impl ScrollView {
    pub fn new(child: Box<dyn Component>, options: ScrollViewOptions) -> Self {
        Self {
            child,
            state: ScrollHandle::new(options),
        }
    }
    pub fn state(&self) -> ScrollHandle {
        self.state.clone()
    }
    pub fn scroll_top(&self) -> usize {
        self.state.snapshot().scroll_top
    }
    pub fn is_following_end(&self) -> bool {
        self.state.snapshot().following_end
    }
    pub fn viewport_height(&self) -> usize {
        self.state.snapshot().viewport_height
    }
    pub fn content_height(&self) -> usize {
        self.state.snapshot().content_height
    }
    pub fn scrollbar(&self) -> ScrollViewScrollbar {
        self.state.snapshot().scrollbar
    }
    pub fn is_scrollbar_visible(&self) -> bool {
        self.state.snapshot().scrollbar_visible
    }
    pub fn is_scrollbar_active(&self) -> bool {
        self.state.snapshot().scrollbar_active
    }
    pub fn content_width(&self, width: usize) -> usize {
        self.state.content_width(width)
    }
    pub fn set_scrollbar(&mut self, bar: ScrollViewScrollbar) {
        self.state.set_scrollbar(bar);
    }
    pub fn set_scrollbar_active(&mut self, active: bool) {
        self.state.set_scrollbar_active(active);
    }
    pub fn scroll_to(&mut self, top: usize, options: ScrollViewScrollToOptions) {
        self.state.scroll_to(top, options);
    }
    pub fn scroll_to_number(&mut self, top: f64, options: ScrollViewScrollToOptions) {
        self.state.scroll_to_number(top, options);
    }
    pub fn scroll_by(&mut self, lines: i64) -> i64 {
        self.state.scroll_by(lines)
    }
    pub fn scroll_by_number(&mut self, lines: f64) -> f64 {
        self.state.scroll_by_number(lines)
    }
    pub fn scroll_to_start(&mut self) {
        self.state.scroll_to_start();
    }
    pub fn scroll_to_end(&mut self) {
        self.state.scroll_to_end();
    }
    pub fn update_layout(&mut self, content_height: usize, viewport_height: usize) {
        self.state
            .mutate(|s, _| s.update_layout(content_height, viewport_height));
    }
    pub fn child(&mut self) -> &mut Box<dyn Component> {
        &mut self.child
    }
}
impl Component for ScrollView {
    fn prepare_mouse_children(&mut self) {
        ensure_handle(&mut self.child);
    }
    fn is_container_component(&self) -> bool {
        true
    }
    fn uses_container_mouse_handler(&self) -> bool {
        true
    }
    fn mouse_action(&mut self, event: &TuiMouseEvent) -> Option<MouseAction> {
        if !valid_container_row(event) {
            return None;
        }
        let child = ensure_handle(&mut self.child);
        // Inherited Container fallback uses event.width, NOT content_width.
        let height = self.child.render(event.width).len();
        container_mouse_action(&[(child, height)], event)
    }
    fn render(&mut self, width: usize) -> Vec<String> {
        let content_width = self.content_width(width);
        let lines = self.child.render(content_width);
        if content_width == width {
            lines
        } else {
            lines.into_iter().map(|line| format!("{line} ")).collect()
        }
    }
    fn render_layout_lines(&mut self, width: usize) -> RenderedLines {
        let content_width = self.content_width(width);
        let lines = self.child.render_layout_lines(content_width);
        if content_width == width {
            lines
        } else {
            lines.map_present(|line| format!("{line} "))
        }
    }
    fn layout_node_mut(&mut self) -> Option<LayoutNode<'_>> {
        Some(LayoutNode::Scroll {
            component: &mut *self.child,
            state: self.state.clone(),
        })
    }
    fn invalidate(&mut self) {
        self.child.invalidate();
    }
}
