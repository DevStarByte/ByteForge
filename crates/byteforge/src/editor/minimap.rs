//! Minimap: the whole document in miniature at the right edge, each character a small block in its
//! syntax color, with a slider for the visible part. Click or drag it to scroll.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bf_text::Point;
use gpui::{Bounds, Corners, Hsla, Window, fill, point, px, quad, size};

use super::EditorView;
use crate::theme::Theme;

/// Height of one line and width of one character in the minimap, in pixels.
pub(crate) const ROW: f32 = 2.0;
pub(crate) const COL: f32 = 1.0;
/// Space kept right of the minimap for the vertical scrollbar.
pub(crate) const SCROLLBAR_GAP: f32 = 14.0;

/// What the minimap shows: the first display row and, per row, colored runs (start column, length).
pub(crate) struct MiniMap {
    pub first_row: usize,
    pub rows: Vec<Vec<(u16, u16, Hsla)>>,
    key: (u64, usize, usize, usize),
    built: Instant,
}

impl EditorView {
    /// Left edge of the minimap in window pixels.
    pub(crate) fn minimap_left(&self) -> f32 {
        let l = &self.layout;
        l.left + l.width - SCROLLBAR_GAP - l.minimap
    }

    /// Display rows the minimap can show at once.
    fn minimap_capacity(&self) -> usize {
        (self.layout.height / ROW).floor().max(1.0) as usize
    }

    /// The first display row shown: the minimap scrolls along when the document doesn't fit.
    pub(crate) fn minimap_first_row(&self) -> usize {
        let rows = self.folds.row_count(self.state.buffer.line_count());
        let capacity = self.minimap_capacity();
        if rows <= capacity {
            return 0;
        }
        let lh = self.layout.line_height.max(1.0);
        let max_scroll = ((rows as f32 - 1.0) * lh).max(1.0);
        ((self.scroll.1 / max_scroll).clamp(0.0, 1.0) * (rows - capacity) as f32).round() as usize
    }

    /// Rebuilds the cached block runs when the text, the scroll position or the size changed (and every
    /// few hundred milliseconds, to pick up new syntax colors).
    pub(crate) fn refresh_minimap(&mut self, theme: &Theme) {
        if self.layout.minimap <= 0.0 {
            self.minimap = None;
            return;
        }
        let first = self.minimap_first_row();
        let capacity = self.minimap_capacity();
        let key = (self.state.buffer.version(), first, capacity, theme.version);
        if let Some(m) = &self.minimap
            && m.key == key
            && m.built.elapsed() < Duration::from_millis(300)
        {
            return;
        }
        let max_cols = (self.layout.minimap / COL).floor() as usize;
        let foreground = theme.c("editor.foreground");
        let line_count = self.state.buffer.line_count();
        let rows_total = self.folds.row_count(line_count);
        let mut rows = Vec::with_capacity(capacity.min(rows_total));
        for row in first..(first + capacity).min(rows_total) {
            let line = self.folds.line_of(row);
            if line >= line_count {
                break;
            }
            let text = self.state.buffer.line(line).into_owned();
            let kinds = self.line_kinds(line, text.len());
            let mut runs: Vec<(u16, u16, Hsla)> = Vec::new();
            let mut col = 0usize;
            for (byte, ch) in text.char_indices() {
                if col >= max_cols {
                    break;
                }
                if ch == '\t' {
                    col = (col / self.state.tab_size + 1) * self.state.tab_size;
                    continue;
                }
                if !ch.is_whitespace() {
                    let mut color = kinds.get(byte).copied().flatten().and_then(|k| theme.token(k)).map_or(foreground, |t| t.color);
                    color.a *= 0.7;
                    match runs.last_mut() {
                        Some((start, len, c)) if *start as usize + *len as usize == col && *c == color => *len += 1,
                        _ => runs.push((col as u16, 1, color)),
                    }
                }
                col += 1;
            }
            rows.push(runs);
        }
        self.minimap = Some(Arc::new(MiniMap { first_row: first, rows, key, built: Instant::now() }));
    }

    /// Scrolls so the display row under `y` (window pixels) in the minimap is centered.
    pub(crate) fn minimap_scroll_to(&mut self, y: f32) {
        let row = self.minimap_first_row() as f32 + ((y - self.layout.top) / ROW).max(0.0);
        let lh = self.layout.line_height.max(1.0);
        let target = row * lh - self.layout.height / 2.0;
        self.scroll_to((self.scroll.0, target), false);
    }

    pub(crate) fn in_minimap(&self, x: f32, y: f32) -> bool {
        self.layout.minimap > 0.0 && x >= self.minimap_left() && x < self.minimap_left() + self.layout.minimap && y < self.layout.top + self.layout.height
    }
}

/// Paints the minimap: caret line, selection, blocks and the slider.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint(
    window: &mut Window,
    theme: &Theme,
    map: &MiniMap,
    bounds: Bounds<gpui::Pixels>,
    scroll_row: f32,
    visible_rows: f32,
    caret_row: Option<usize>,
    selection_rows: Option<(usize, usize)>,
    hovered: bool,
) {
    let left = f32::from(bounds.left());
    let top = f32::from(bounds.top());
    let width = f32::from(bounds.size.width);
    if theme.has("minimap.background") {
        window.paint_quad(fill(bounds, theme.bg("minimap.background")));
    }
    let y_of = |row: usize| top + (row as f32 - map.first_row as f32) * ROW;
    if let Some((s, e)) = selection_rows {
        let color = theme.c_or("minimap.selectionHighlight", 0x264f78c0);
        let (y0, y1) = (y_of(s).max(top), y_of(e + 1).min(top + f32::from(bounds.size.height)));
        if y1 > y0 {
            window.paint_quad(fill(Bounds::new(point(px(left), px(y0)), size(px(width), px(y1 - y0))), color));
        }
    }
    if let Some(row) = caret_row
        && row >= map.first_row
        && row < map.first_row + map.rows.len()
    {
        let color = theme.c_or("minimap.caretLine", 0x80808050);
        window.paint_quad(fill(Bounds::new(point(px(left), px(y_of(row))), size(px(width), px(ROW))), color));
    }
    for (i, runs) in map.rows.iter().enumerate() {
        let y = top + i as f32 * ROW;
        for &(start, len, color) in runs {
            let rect = Bounds::new(point(px(left + start as f32 * COL), px(y)), size(px(len as f32 * COL), px(ROW - 0.5)));
            window.paint_quad(fill(rect, color));
        }
    }
    // Slider over the visible part.
    let key = if hovered { "minimapSlider.hoverBackground" } else { "minimapSlider.background" };
    let color = theme.c_or(key, if hovered { 0x64646459 } else { 0x79797933 });
    let y = top + (scroll_row - map.first_row as f32) * ROW;
    let h = (visible_rows * ROW).max(4.0);
    window.paint_quad(quad(Bounds::new(point(px(left), px(y)), size(px(width), px(h))), Corners::all(px(0.)), color, 0.0, Hsla::default(), gpui::BorderStyle::Solid));
}

/// The selection's display rows, for the minimap highlight.
pub(crate) fn selection_rows(view: &EditorView) -> Option<(usize, usize)> {
    if !view.state.has_selection() {
        return None;
    }
    let (s, e): (Point, Point) = view.state.selection();
    Some((view.folds.row_of(s.line), view.folds.row_of(e.line)))
}
