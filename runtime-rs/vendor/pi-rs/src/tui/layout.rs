//! Viewport measurement, layout, painting and visual hit testing, ported from
//! upstream layout.ts. Layout boxes live in an owned arena with index parents:
//! no self-referential borrows, unsafe code or raw-pointer component identities.
//! Coordinates are integral terminal cells; OS input/focus/render-loop wiring
//! and arbitrary JS-number/UTF-16 component boundaries are separate work.
use crate::tui::component::{Component, CURSOR_MARKER};
use crate::tui::component_mouse::ComponentHandle;
use crate::tui::components::scroll_view::{RequestRender, ScrollHandle, ScrollViewScrollbar};
use crate::tui::components::stack::{
    allocate_stack_sizes, visible_stack_entries, LayoutViewport, StackAlign, StackBasis, StackKind,
};
use crate::tui::layout_node::{ComponentCacheId, LayoutNode};
use crate::tui::overlay::composite_tui_line;
use crate::tui::rendered_lines::RenderedLines;
use crate::tui::terminal_image::{crop_kitty_image_line, get_kitty_image_metadata, is_image_line};
use crate::tui::utils::{
    extract_ansi_code, get_active_background_ansi, get_grapheme_cell_range, slice_by_column,
    visible_width,
};
use std::collections::{BTreeMap, HashMap};

pub type LayoutBoxId = usize;
pub type ComponentPath = Vec<usize>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayoutRect {
    pub x: i64,
    pub y: i64,
    pub width: usize,
    pub height: usize,
}
impl LayoutRect {
    fn right(self) -> i64 {
        self.x.saturating_add(self.width as i64)
    }
    fn bottom(self) -> i64 {
        self.y.saturating_add(self.height as i64)
    }
    fn intersect(self, other: Self) -> Self {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        Self {
            x,
            y,
            width: self.right().min(other.right()).saturating_sub(x).max(0) as usize,
            height: self.bottom().min(other.bottom()).saturating_sub(y).max(0) as usize,
        }
    }
    pub fn contains(self, x: i64, y: i64) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
}
#[derive(Clone, Debug)]
pub struct LayoutBox {
    /// Original child indices, including invisible entries. Resolve in the same
    /// component tree; not a promise of persistent identity after tree mutation.
    pub component_path: ComponentPath,
    /// Retains the actual object for interactive frames; paths remain diagnostic.
    /// None for legacy borrow-only components. Never re-resolve a stale path.
    pub component: Option<ComponentHandle>,
    pub rect: LayoutRect,
    pub clip: LayoutRect,
    pub children: Vec<LayoutBoxId>,
    pub parent: Option<LayoutBoxId>,
    pub lines: Option<RenderedLines>,
    pub line_offset: Option<usize>,
    pub scroll_view: Option<ScrollHandle>,
    pub scroll_content_lines: Option<RenderedLines>,
    pub layer: i32,
}
impl LayoutBox {
    fn new(path: &[usize], rect: LayoutRect, parent_clip: LayoutRect) -> Self {
        Self {
            component_path: path.to_vec(),
            component: None,
            rect,
            clip: parent_clip.intersect(rect),
            children: Vec::new(),
            parent: None,
            lines: None,
            line_offset: None,
            scroll_view: None,
            scroll_content_lines: None,
            layer: 0,
        }
    }
}
#[derive(Clone, Debug)]
pub struct LayoutFrame {
    pub root: LayoutBoxId,
    pub boxes: Vec<LayoutBox>,
    pub width: usize,
    pub height: usize,
    /// Usually dense. Upstream carried-image painting may extend this past the
    /// viewport with absent rows; sparse output preserves those holes.
    pub lines: RenderedLines,
    pub primary_scroll_view: Option<ScrollHandle>,
}
impl LayoutFrame {
    pub fn root_box(&self) -> &LayoutBox {
        &self.boxes[self.root]
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScrollbarGeometry {
    pub column: i64,
    pub track_top: i64,
    pub track_height: usize,
    pub thumb_top: i64,
    pub thumb_height: usize,
    pub max_scroll_top: usize,
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum CacheIdentity {
    Owned(ComponentPath),
    Shared(ComponentCacheId),
}
struct Context {
    viewport: LayoutViewport,
    cache: HashMap<(CacheIdentity, usize), RenderedLines>,
    boxes: Vec<LayoutBox>,
    request_render: RequestRender,
    primary: Option<ScrollHandle>,
}
fn child_path(path: &[usize], index: usize) -> ComponentPath {
    let mut path = path.to_vec();
    path.push(index);
    path
}
impl Context {
    fn cached(
        &mut self,
        component: &mut dyn Component,
        path: &[usize],
        width: usize,
    ) -> RenderedLines {
        let cache_id = component.layout_cache_id();
        self.cached_with_id(component, path, width, cache_id)
    }
    fn cached_with_id(
        &mut self,
        component: &mut dyn Component,
        path: &[usize],
        width: usize,
        cache_id: Option<ComponentCacheId>,
    ) -> RenderedLines {
        let width = width.max(1);
        let identity = cache_id.map_or_else(
            || CacheIdentity::Owned(path.to_vec()),
            CacheIdentity::Shared,
        );
        self.cache
            .entry((identity, width))
            .or_insert_with(|| component.render_layout_lines(width))
            .clone()
    }
    fn push(&mut self, b: LayoutBox) -> LayoutBoxId {
        let i = self.boxes.len();
        self.boxes.push(b);
        i
    }
    fn adopt(&mut self, parent: LayoutBoxId, child: LayoutBoxId) {
        self.boxes[child].parent = Some(parent);
        self.boxes[parent].children.push(child);
    }
    fn translate(&mut self, index: LayoutBoxId, delta: i64) {
        self.boxes[index].rect.y += delta;
        for child in self.boxes[index].children.clone() {
            self.translate(child, delta);
        }
    }
    fn update_clips(&mut self, index: LayoutBoxId, clip: LayoutRect) {
        let clip = clip.intersect(self.boxes[index].rect);
        self.boxes[index].clip = clip;
        for child in self.boxes[index].children.clone() {
            self.update_clips(child, clip);
        }
    }
    fn layout(
        &mut self,
        component: &mut dyn Component,
        path: &[usize],
        rect: LayoutRect,
        height: Option<usize>,
        clip: LayoutRect,
    ) -> LayoutBoxId {
        if let Some(handle) = component.component_handle() {
            let cache_id = handle.layout_cache_id();
            let id = handle.with_mut(|concrete| {
                concrete.prepare_mouse_children();
                self.layout_inner(concrete, path, rect, height, clip, cache_id)
            });
            self.boxes[id].component = Some(handle);
            id
        } else {
            let cache_id = component.layout_cache_id();
            self.layout_inner(component, path, rect, height, clip, cache_id)
        }
    }
    fn layout_inner(
        &mut self,
        component: &mut dyn Component,
        path: &[usize],
        rect: LayoutRect,
        height: Option<usize>,
        clip: LayoutRect,
        cache_id: Option<ComponentCacheId>,
    ) -> LayoutBoxId {
        let LayoutRect { x, y, width, .. } = rect;
        let width = width.max(1);
        match component.layout_node_mut() {
            None => {
                let lines = self.cached_with_id(component, path, width, cache_id);
                let allocated = height.unwrap_or(lines.len());
                let mut offset = 0;
                if lines.len() > allocated && allocated > 0 {
                    if let Some((cursor, _)) = lines
                        .present()
                        .find(|(_, line)| line.contains(CURSOR_MARKER))
                    {
                        if cursor >= allocated {
                            offset = cursor - allocated + 1;
                        }
                    }
                }
                let mut b = LayoutBox::new(
                    path,
                    LayoutRect {
                        x,
                        y,
                        width,
                        height: allocated,
                    },
                    clip,
                );
                b.lines = Some(lines);
                b.line_offset = Some(offset);
                self.push(b)
            }
            Some(LayoutNode::Scroll {
                component: child,
                state,
            }) => {
                let previous = state.snapshot().scroll_top;
                let content_width = state.content_width(width);
                let child_path = child_path(path, 0);
                let child_id = self.layout(
                    child,
                    &child_path,
                    LayoutRect {
                        x,
                        y: y - previous as i64,
                        width: content_width,
                        height: 0,
                    },
                    None,
                    clip,
                );
                let content_height = self.boxes[child_id].rect.height;
                let allocated = height.unwrap_or(content_height);
                state.update_layout(content_height, allocated, self.request_render.clone());
                let snapshot = state.snapshot();
                self.translate(child_id, previous as i64 - snapshot.scroll_top as i64);
                if snapshot.primary || self.primary.is_none() {
                    self.primary = Some(state.clone());
                }
                let mut b = LayoutBox::new(
                    path,
                    LayoutRect {
                        x,
                        y,
                        width,
                        height: allocated,
                    },
                    clip,
                );
                b.scroll_view = Some(state);
                b.scroll_content_lines = Some(self.cached(child, &child_path, content_width));
                let clip = b.clip;
                let id = self.push(b);
                self.adopt(id, child_id);
                self.update_clips(child_id, clip);
                id
            }
            Some(LayoutNode::Stack {
                kind,
                entries,
                gap,
                align,
            }) => {
                let indices = visible_stack_entries(entries, &self.viewport);
                let options: Vec<_> = indices
                    .iter()
                    .map(|&i| entries[i].options.clone())
                    .collect();
                let mut intrinsic = Vec::new();
                for &i in &indices {
                    let value = match entries[i].options.basis {
                        Some(StackBasis::Size(size)) => size,
                        _ => {
                            let lines = self.cached(
                                &mut *entries[i].component,
                                &child_path(path, i),
                                width,
                            );
                            if kind == StackKind::Vertical {
                                lines.len() as f64
                            } else {
                                lines
                                    .present()
                                    .map(|(_, line)| visible_width(line))
                                    .max()
                                    .unwrap_or(0) as f64
                            }
                        }
                    };
                    intrinsic.push(value);
                }
                let available = if kind == StackKind::Vertical {
                    height.map(|n| n as f64)
                } else {
                    Some(width as f64)
                };
                let sizes = allocate_stack_sizes(&options, &intrinsic, available, gap);
                if kind == StackKind::Vertical {
                    let natural =
                        sizes.iter().sum::<f64>() + indices.len().saturating_sub(1) as f64 * gap;
                    let allocated = height.unwrap_or(natural as usize);
                    let id = self.push(LayoutBox::new(
                        path,
                        LayoutRect {
                            x,
                            y,
                            width,
                            height: allocated,
                        },
                        clip,
                    ));
                    let clip = self.boxes[id].clip;
                    let mut child_y = y;
                    for (position, &i) in indices.iter().enumerate() {
                        let size = sizes[position] as usize;
                        let child = self.layout(
                            &mut *entries[i].component,
                            &child_path(path, i),
                            LayoutRect {
                                x,
                                y: child_y,
                                width,
                                height: size,
                            },
                            Some(size),
                            clip,
                        );
                        self.adopt(id, child);
                        child_y += size as i64 + gap as i64;
                    }
                    id
                } else {
                    let mut heights = Vec::new();
                    for (position, &i) in indices.iter().enumerate() {
                        heights.push(
                            self.cached(
                                &mut *entries[i].component,
                                &child_path(path, i),
                                (sizes[position] as usize).max(1),
                            )
                            .len(),
                        );
                    }
                    let allocated =
                        height.unwrap_or_else(|| heights.iter().copied().max().unwrap_or(0));
                    let id = self.push(LayoutBox::new(
                        path,
                        LayoutRect {
                            x,
                            y,
                            width,
                            height: allocated,
                        },
                        clip,
                    ));
                    let clip = self.boxes[id].clip;
                    let mut child_x = x;
                    for (position, &i) in indices.iter().enumerate() {
                        let child_height = if align == StackAlign::Stretch {
                            allocated
                        } else {
                            allocated.min(heights[position])
                        };
                        let child_y = y + match align {
                            StackAlign::Center => ((allocated - child_height) / 2) as i64,
                            StackAlign::End => (allocated - child_height) as i64,
                            _ => 0,
                        };
                        let child_width = sizes[position] as usize;
                        let child_rect = LayoutRect {
                            x: child_x,
                            y: child_y,
                            width: child_width,
                            height: child_height,
                        };
                        let path = child_path(path, i);
                        let child = if child_width == 0 {
                            let mut b = LayoutBox::new(&path, child_rect, clip);
                            b.component = entries[i].component.component_handle();
                            b.clip = LayoutRect {
                                x: child_x,
                                y: child_y,
                                width: 0,
                                height: 0,
                            };
                            self.push(b)
                        } else {
                            self.layout(
                                &mut *entries[i].component,
                                &path,
                                child_rect,
                                Some(child_height),
                                clip,
                            )
                        };
                        self.adopt(id, child);
                        child_x += child_width as i64 + gap as i64;
                    }
                    id
                }
            }
        }
    }
}

pub fn get_scrollbar_geometry(
    frame: &LayoutFrame,
    index: LayoutBoxId,
    include_hidden_auto: bool,
) -> Option<ScrollbarGeometry> {
    let b = frame.boxes.get(index)?;
    let state = b.scroll_view.as_ref()?.snapshot();
    if b.rect.width == 0 || b.rect.height == 0 {
        return None;
    }
    let content = b
        .children
        .first()
        .map(|&i| frame.boxes[i].rect.height)
        .unwrap_or_else(|| {
            b.scroll_content_lines
                .as_ref()
                .map_or(0, RenderedLines::len)
        });
    let track = b.rect.height;
    let can_reveal =
        include_hidden_auto && state.scrollbar == ScrollViewScrollbar::Auto && content > track;
    if !state.scrollbar_visible && !can_reveal {
        return None;
    }
    let thumb = ((track as f64 * track as f64 / content as f64)
        .round()
        .min(track as f64) as usize)
        .max(track.min(2));
    let max = content.saturating_sub(track);
    let offset = if max == 0 {
        0
    } else {
        (state.scroll_top as f64 / max as f64 * (track - thumb) as f64).round() as i64
    };
    let column = b.rect.right() - 1;
    if column < b.clip.x || column >= b.clip.right() {
        return None;
    }
    Some(ScrollbarGeometry {
        column,
        track_top: b.rect.y,
        track_height: track,
        thumb_top: b.rect.y + offset,
        thumb_height: thumb,
        max_scroll_top: max,
    })
}
fn replace_scrollbar_cell(
    line: &str,
    column: usize,
    total_width: usize,
    replacement: &str,
    preserve_background: bool,
) -> String {
    if is_image_line(line) {
        return line.into();
    }
    let (start, end) = get_grapheme_cell_range(line, column).unwrap_or((column, column + 1));
    let before = slice_by_column(line, 0, start, true);
    let target = slice_by_column(line, start, end - start, true);
    let after = slice_by_column(line, end, total_width.saturating_sub(end), true);
    let mut prefix_end = 0;
    while let Some(ansi) = extract_ansi_code(&target, prefix_end) {
        prefix_end += ansi.len();
    }
    let background = if preserve_background {
        get_active_background_ansi(&target[..prefix_end])
    } else {
        String::new()
    };
    format!(
        "{before}{}\x1b[0m\x1b]8;;\x07{background}{}{replacement}{}{after}",
        " ".repeat(start.saturating_sub(visible_width(&before))),
        " ".repeat(column.saturating_sub(start)),
        " ".repeat(end.saturating_sub(column + 1))
    )
}
fn strip_zone_prefix(mut line: &str) -> &str {
    loop {
        let Some(rest) = line.strip_prefix("\x1b]133;") else {
            return line;
        };
        let Some(zone) = rest.as_bytes().first() else {
            return line;
        };
        if !matches!(zone, b'A' | b'B' | b'C') {
            return line;
        }
        if let Some(tail) = rest[1..]
            .strip_prefix('\x07')
            .or_else(|| rest[1..].strip_prefix("\x1b\\"))
        {
            line = tail;
        } else {
            return line;
        }
    }
}
// A carried Kitty image can be assigned to an offscreen positive row by the
// upstream painter. Preserve that array growth/holes without allocating every
// intermediate row, and ignore negative array properties as JSON/rendering do.
struct Canvas {
    rows: Vec<String>,
    overflow: BTreeMap<usize, String>,
    length: usize,
}
impl Canvas {
    fn new(height: usize) -> Self {
        Self {
            rows: vec![String::new(); height],
            overflow: BTreeMap::new(),
            length: height,
        }
    }
    fn len(&self) -> usize {
        self.length
    }
    fn get(&self, row: usize) -> Option<&str> {
        self.rows
            .get(row)
            .or_else(|| self.overflow.get(&row))
            .map(String::as_str)
    }
    fn set(&mut self, row: usize, line: String) {
        if let Some(target) = self.rows.get_mut(row) {
            *target = line;
        } else {
            self.overflow.insert(row, line);
            self.length = self.length.max(row + 1);
        }
    }
    fn finish(self) -> RenderedLines {
        if self.overflow.is_empty() {
            RenderedLines::dense(self.rows)
        } else {
            let mut lines: BTreeMap<_, _> = self.rows.into_iter().enumerate().collect();
            lines.extend(self.overflow);
            RenderedLines::sparse(self.length, lines)
        }
    }
}
fn paint_box(frame: &LayoutFrame, index: LayoutBoxId, screen: &mut Canvas) {
    let b = &frame.boxes[index];
    if let Some(lines) = &b.lines {
        let first = b.rect.y.max(b.clip.y).max(0);
        let last = b
            .rect
            .bottom()
            .min(b.clip.bottom())
            .min(screen.len() as i64);
        let offset = b.line_offset.unwrap_or(0);
        for row in first..last {
            let Some(source) = lines.get(offset + (row - b.rect.y) as usize) else {
                continue;
            };
            let mut line = strip_zone_prefix(source).to_owned();
            if let Some(image) = get_kitty_image_metadata(&line) {
                let visible = image.rows.min(
                    (screen.len() as i64)
                        .min(b.clip.bottom())
                        .saturating_sub(row)
                        .max(0) as usize,
                );
                if visible < image.rows {
                    line = crop_kitty_image_line(&line, 0, visible as i64);
                }
            }
            let target = screen.get(row as usize).unwrap_or("");
            let painted = if b.rect.x == 0
                && b.rect.width >= frame.width
                && (is_image_line(&line) || target.is_empty())
            {
                line
            } else {
                composite_tui_line(
                    target,
                    &line,
                    b.rect.x.max(0) as usize,
                    b.rect.width,
                    frame.width,
                )
            };
            screen.set(row as usize, painted);
        }
    }
    for &child in &b.children {
        paint_box(frame, child, screen);
    }
    if let (Some(scroll), Some(lines)) = (&b.scroll_view, &b.scroll_content_lines) {
        let top = scroll.snapshot().scroll_top;
        if top > 0 && b.rect.height > 0 {
            if let Some((image_row, line)) = lines.last_nonempty_before(top) {
                if let Some(metadata) = get_kitty_image_metadata(line) {
                    let hidden = top - image_row;
                    if hidden < metadata.rows
                        && b.rect.x == 0
                        && b.rect.width >= frame.width
                        && b.rect.y >= 0
                    {
                        let cropped = crop_kitty_image_line(
                            line,
                            hidden as i64,
                            b.rect.height.min(metadata.rows - hidden) as i64,
                        );
                        screen.set(b.rect.y as usize, cropped);
                    }
                }
            }
        }
    }
    if let Some(geometry) = get_scrollbar_geometry(frame, index, false) {
        let scroll = b.scroll_view.as_ref().unwrap();
        let first = geometry.track_top.max(b.clip.y).max(0);
        let last = (geometry.track_top + geometry.track_height as i64)
            .min(b.clip.bottom())
            .min(screen.len() as i64);
        for row in first..last {
            let thumb = row >= geometry.thumb_top
                && row < geometry.thumb_top + geometry.thumb_height as i64;
            let state = scroll.snapshot();
            let glyph = if thumb {
                if state.scrollbar_active {
                    "█"
                } else {
                    "┃"
                }
            } else {
                "│"
            };
            let replacement = scroll.style_scrollbar(thumb, glyph);
            let painted = replace_scrollbar_cell(
                screen.get(row as usize).unwrap_or(""),
                geometry.column.max(0) as usize,
                frame.width,
                &replacement,
                state.scrollbar != ScrollViewScrollbar::Always,
            );
            screen.set(row as usize, painted);
        }
    }
}

pub fn render_layout_frame(
    root: &mut dyn Component,
    width: usize,
    height: usize,
    request_render: RequestRender,
) -> LayoutFrame {
    let width = width.max(1);
    let height = height.max(1);
    let mut context = Context {
        viewport: LayoutViewport {
            width,
            height: height as u64,
        },
        cache: HashMap::new(),
        boxes: Vec::new(),
        request_render,
        primary: None,
    };
    let rect = LayoutRect {
        x: 0,
        y: 0,
        width,
        height,
    };
    let root = context.layout(root, &[], rect, Some(height), rect);
    let mut frame = LayoutFrame {
        root,
        boxes: context.boxes,
        width,
        height,
        lines: RenderedLines::dense(Vec::new()),
        primary_scroll_view: context.primary,
    };
    let mut lines = Canvas::new(height);
    paint_box(&frame, root, &mut lines);
    frame.lines = lines.finish();
    frame
}
fn hit_indices(frame: &LayoutFrame, x: i64, y: i64) -> Vec<(LayoutBoxId, usize)> {
    fn visit(
        frame: &LayoutFrame,
        index: LayoutBoxId,
        x: i64,
        y: i64,
        depth: usize,
        out: &mut Vec<(LayoutBoxId, usize)>,
    ) {
        let b = &frame.boxes[index];
        if !b.clip.contains(x, y) {
            return;
        }
        out.push((index, depth));
        for &child in &b.children {
            visit(frame, child, x, y, depth + 1, out);
        }
    }
    let mut result = Vec::new();
    visit(frame, frame.root, x, y, 0, &mut result);
    result
}
/// Deepest visual boxes first; layer takes priority over depth (stable ties).
pub fn get_layout_boxes_at(frame: &LayoutFrame, x: i64, y: i64) -> Vec<&LayoutBox> {
    let mut result = hit_indices(frame, x, y);
    result.sort_by(|(a, ad), (b, bd)| {
        frame.boxes[*b]
            .layer
            .cmp(&frame.boxes[*a].layer)
            .then(bd.cmp(ad))
    });
    result.into_iter().map(|(i, _)| &frame.boxes[i]).collect()
}
pub fn get_scroll_view_box_id(frame: &LayoutFrame, scroll: &ScrollHandle) -> Option<LayoutBoxId> {
    fn visit(
        frame: &LayoutFrame,
        index: LayoutBoxId,
        scroll: &ScrollHandle,
    ) -> Option<LayoutBoxId> {
        let b = &frame.boxes[index];
        if b.scroll_view.as_ref() == Some(scroll) {
            return Some(index);
        }
        b.children
            .iter()
            .find_map(|&child| visit(frame, child, scroll))
    }
    visit(frame, frame.root, scroll)
}
pub fn get_scroll_view_box<'a>(
    frame: &'a LayoutFrame,
    scroll: &ScrollHandle,
) -> Option<&'a LayoutBox> {
    get_scroll_view_box_id(frame, scroll).map(|i| &frame.boxes[i])
}
pub fn get_scroll_views_at(frame: &LayoutFrame, x: i64, y: i64) -> Vec<ScrollHandle> {
    let mut result: Vec<_> = hit_indices(frame, x, y)
        .into_iter()
        .filter(|(i, _)| {
            frame.boxes[*i].scroll_view.is_some() && frame.boxes[*i].rect.contains(x, y)
        })
        .collect();
    result.sort_by(|(_, ad), (_, bd)| bd.cmp(ad));
    result
        .into_iter()
        .map(|(i, _)| frame.boxes[i].scroll_view.as_ref().unwrap().clone())
        .collect()
}
