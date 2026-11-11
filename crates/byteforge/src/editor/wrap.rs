//! Word wrap: long lines continue on the next screen rows instead of running off to the right.
//!
//! The editor font is monospaced, so wrapping is column arithmetic: with `cols` columns fitting in the
//! text area, a line whose visual length (tabs expanded, inlay hints included) is `n` takes
//! `max(1, ceil(n / cols))` rows, and visual column `v` sits on row `v / cols` of its line at
//! `v % cols`. The [`WrapMap`] keeps the first row of every line; everything that turns positions into
//! screen coordinates (painting, clicks, the caret, scrolling, Up/Down) goes through it. Folded lines
//! take no rows.

use std::sync::Arc;

use super::*;

/// The screen row each line starts on, for one text, width, set of folds and inlay hints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WrapMap {
    pub cols: usize,
    /// `starts[l]` is the first row of line `l`; `starts[line_count]` is the number of rows.
    pub starts: Arc<Vec<usize>>,
    key: (u64, usize, u64),
}

/// Rows a line of `visual_len` columns takes.
pub(crate) fn rows_for(visual_len: usize, cols: usize) -> usize {
    visual_len.div_ceil(cols.max(1)).max(1)
}

/// The segment (row within its line) and column on that row of visual column `v` in a line of
/// `visual_len` columns. The end of a line that exactly fills its rows stays on the last row.
pub(crate) fn segment_of(v: usize, visual_len: usize, cols: usize) -> (usize, usize) {
    let cols = cols.max(1);
    let seg = (v / cols).min(rows_for(visual_len, cols) - 1);
    (seg, v - seg * cols)
}

/// The first rows of lines that take `rows[l]` rows each.
pub(crate) fn starts_from(rows: impl Iterator<Item = usize>) -> Vec<usize> {
    let mut starts = vec![0];
    let mut acc = 0;
    for r in rows {
        acc += r;
        starts.push(acc);
    }
    starts
}

/// The line and segment shown on `row`.
pub(crate) fn line_at(starts: &[usize], row: usize) -> (usize, usize) {
    let lines = starts.len().saturating_sub(1);
    if lines == 0 {
        return (0, 0);
    }
    // The last line whose first row is at or before `row` (and that has rows: folded lines have none).
    let mut line = starts.partition_point(|&s| s <= row).saturating_sub(1).min(lines - 1);
    while line > 0 && starts[line + 1] == starts[line] {
        line -= 1;
    }
    let seg = row.saturating_sub(starts[line]).min((starts[line + 1] - starts[line]).saturating_sub(1));
    (line, seg)
}

impl EditorView {
    /// Columns per row while wrapping (`None` when word wrap is off or the size isn't known yet).
    pub(crate) fn wrap_cols(&self) -> Option<usize> {
        self.wrap.as_ref().map(|w| w.cols)
    }

    /// A line's width in columns: tabs expanded, inlay hints included.
    pub(crate) fn visual_len(&self, line: usize) -> usize {
        let text = self.state.buffer.line(line);
        let len = text.chars().count();
        bf_text::visual_col(&text, len, self.state.tab_size) + self.inlay_shift(line, len, false)
    }

    /// Rebuilds the wrap map when wrapping and the text, width, folds or inlay hints changed (every frame).
    pub(crate) fn update_wrap(&mut self) {
        if !self.word_wrap || self.layout.char_width <= 0.0 || self.layout.width <= 0.0 {
            self.wrap = None;
            return;
        }
        // One column of slack so the caret at the end of a full row stays visible.
        let cols = ((self.layout.text_width() / self.layout.char_width).floor() as usize).saturating_sub(1).max(10);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(&self.folds.hidden_ranges(), &mut hasher);
        std::hash::Hash::hash(&self.inlays, &mut hasher);
        let key = (self.state.buffer.version(), cols, std::hash::Hasher::finish(&hasher));
        if self.wrap.as_ref().is_some_and(|w| w.key == key) {
            return;
        }
        let rows = (0..self.state.buffer.line_count()).map(|l| if self.folds.is_hidden(l) { 0 } else { rows_for(self.visual_len(l), cols) });
        self.wrap = Some(WrapMap { cols, starts: Arc::new(starts_from(rows)), key });
    }

    /// The first screen row of `line` (a folded line shares the row of the line its fold starts on).
    pub(crate) fn row_of(&self, line: usize) -> usize {
        match &self.wrap {
            Some(w) => {
                let line = match self.folds.hidden_range(line) {
                    Some((s, _)) => s.saturating_sub(1),
                    None => line,
                };
                w.starts.get(line).copied().unwrap_or(0)
            }
            None => self.folds.row_of(line),
        }
    }

    /// Screen rows of the whole text.
    pub(crate) fn row_count(&self) -> usize {
        match &self.wrap {
            Some(w) => w.starts.last().copied().unwrap_or(1).max(1),
            None => self.folds.row_count(self.state.buffer.line_count()),
        }
    }

    /// The line (and its segment) on screen row `row`.
    pub(crate) fn line_at_row(&self, row: usize) -> (usize, usize) {
        match &self.wrap {
            Some(w) => line_at(&w.starts, row),
            None => (self.folds.line_of(row).min(self.state.buffer.line_count() - 1), 0),
        }
    }

    /// Screen row and column (in character cells from the text's left edge) of `p`. With `inclusive`, a
    /// position that has an inlay hint goes after the hint.
    pub(crate) fn screen_pos(&self, p: Point, inclusive: bool) -> (usize, usize) {
        let line = self.state.buffer.line(p.line);
        let v = bf_text::visual_col(&line, p.col, self.state.tab_size) + self.inlay_shift(p.line, p.col, inclusive);
        match self.wrap_cols() {
            Some(cols) => {
                let (seg, x) = segment_of(v, self.visual_len(p.line), cols);
                (self.row_of(p.line) + seg, x)
            }
            None => (self.folds.row_of(p.line), v),
        }
    }

    /// The text position at screen `row`, fractional column `x` (in cells).
    pub(crate) fn point_at_cell(&self, row: usize, x: f32) -> Point {
        let (line, seg) = self.line_at_row(row);
        let visual = match self.wrap_cols() {
            // On a row that isn't a line's last, a click past its end stays on that row.
            Some(cols) => {
                let last_seg = seg + 1 == rows_for(self.visual_len(line), cols);
                let x = if last_seg { x } else { x.min((cols - 1) as f32) };
                (seg * cols) as f32 + x.max(0.0)
            }
            None => x,
        };
        Point::new(line, self.col_at_visual(line, visual))
    }

    /// Up/Down (and Page Up/Down): by screen rows while wrapping with one cursor, by lines otherwise.
    pub(super) fn vertical(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        if self.wrap.is_some() && !self.state.has_multiple_cursors() {
            self.move_rows(delta, extend, cx);
        } else {
            self.mv(cx, |s| s.move_vertical(delta, extend));
        }
    }

    /// Up/Down with word wrap: one screen row, keeping the column the caret had when the moves started.
    pub(super) fn move_rows(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        let caret = self.state.caret();
        let (row, x) = self.screen_pos(caret, true);
        let goal = match self.wrap_goal {
            Some((at, goal)) if at == caret => goal,
            _ => x,
        };
        let target = (row as isize + delta).clamp(0, self.row_count() as isize - 1) as usize;
        let p = if target == row && delta < 0 {
            Point::new(caret.line, 0).min(caret)
        } else if target == row && delta > 0 {
            Point::new(caret.line, self.state.buffer.line_len(caret.line))
        } else {
            self.point_at_cell(target, goal as f32)
        };
        self.state.set_caret(p, extend);
        self.wrap_goal = Some((self.state.caret(), goal));
        self.after_move(cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_and_segments() {
        assert_eq!(rows_for(0, 10), 1);
        assert_eq!(rows_for(10, 10), 1);
        assert_eq!(rows_for(11, 10), 2);
        // The end of a line that fills its row exactly stays on that row.
        assert_eq!(segment_of(10, 10, 10), (0, 10));
        assert_eq!(segment_of(10, 25, 10), (1, 0));
        assert_eq!(segment_of(24, 25, 10), (2, 4));
    }

    #[test]
    fn rows_to_lines_with_folded_lines() {
        // Line 0: 1 row, line 1: 3 rows, line 2: folded (0 rows), line 3: 2 rows.
        let starts = starts_from([1, 3, 0, 2].into_iter());
        assert_eq!(starts, [0, 1, 4, 4, 6]);
        assert_eq!(line_at(&starts, 0), (0, 0));
        assert_eq!(line_at(&starts, 2), (1, 1));
        assert_eq!(line_at(&starts, 3), (1, 2));
        assert_eq!(line_at(&starts, 4), (3, 0));
        assert_eq!(line_at(&starts, 5), (3, 1));
        assert_eq!(line_at(&starts, 99), (3, 1));
    }
}

#[cfg(test)]
mod view_tests {
    use super::*;
    use gpui::AppContext as _;

    /// An editor 20 columns wide (10 px cells, 200 px text area, so 19 columns wrap) with word wrap on.
    fn wrapped(cx: &mut gpui::TestAppContext, text: &str) -> Entity<EditorView> {
        cx.update(|cx| crate::theme::Theme::build(std::sync::Arc::new(bf_core::theme::ThemeData::default()), 0, cx).install(cx));
        let view = cx.new(|cx| EditorView::new(text, None, cx));
        view.update(cx, |v, _| {
            v.word_wrap = true;
            v.layout = Layout { left: 0.0, top: 0.0, width: 240.0, height: 400.0, gutter: 40.0, text_left: 40.0, char_width: 10.0, line_height: 20.0, glyph: 0.0, fold_left: 0.0, fold_width: 0.0, minimap: 0.0 };
            v.update_wrap();
        });
        view
    }

    #[gpui::test]
    fn long_lines_take_several_rows(cx: &mut gpui::TestAppContext) {
        let long = "x".repeat(45);
        let view = wrapped(cx, &format!("short\n{long}\nend"));
        view.update(cx, |v, cx| {
            assert_eq!(v.wrap_cols(), Some(19));
            // 1 + ceil(45 / 19) = 3 + 1 rows.
            assert_eq!(v.row_count(), 5);
            assert_eq!(v.row_of(2), 4);
            assert_eq!(v.screen_pos(Point::new(1, 20), false), (2, 1));
            assert_eq!(v.point_at_cell(3, 2.0), Point::new(1, 40));
            // A click past the end of a row that isn't the line's last stays on that row.
            assert_eq!(v.point_at_cell(1, 30.0).line, 1);
            assert!(v.point_at_cell(1, 30.0).col < 19);

            // Down from the first row of the long line goes to its second row, keeping the column.
            v.state.set_caret(Point::new(1, 5), false);
            v.move_rows(1, false, cx);
            assert_eq!(v.state.caret(), Point::new(1, 24));
            v.move_rows(1, false, cx);
            assert_eq!(v.state.caret(), Point::new(1, 43));
            v.move_rows(1, false, cx);
            assert_eq!(v.state.caret(), Point::new(2, 3), "the short last line: its end");
            v.move_rows(-1, false, cx);
            assert_eq!(v.state.caret(), Point::new(1, 43), "back up: the column it aimed for (5) on the last row");
        });
    }

    #[gpui::test]
    fn wrap_follows_edits_and_turns_off(cx: &mut gpui::TestAppContext) {
        let view = wrapped(cx, "abc");
        view.update(cx, |v, cx| {
            assert_eq!(v.row_count(), 1);
            v.edit(cx, |s| s.type_text(&"y".repeat(30)));
            v.update_wrap();
            assert_eq!(v.row_count(), 2);
            v.word_wrap = false;
            v.update_wrap();
            assert!(v.wrap.is_none());
            assert_eq!(v.row_count(), 1);
        });
    }

    #[gpui::test]
    fn a_wrapped_editor_draws(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::theme::Theme::build(std::sync::Arc::new(bf_core::theme::ThemeData::default()), 0, cx).install(cx));
        let text = format!("fn main() {{\n    let s = \"{}\";\n\tlet t = 1;\n}}\n", "word ".repeat(80));
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut e = EditorView::new(&text, None, cx);
            e.word_wrap = true;
            e
        });
        view.update_in(cx, |v, window, cx| {
            v.focus(window);
            v.state.select(Point::new(1, 3), Point::new(1, 300));
            cx.notify();
        });
        cx.run_until_parked();
        view.update(cx, |v, _| {
            // Prepaint built the map for the window's width: the long line takes several rows.
            let cols = v.wrap_cols().expect("wrapping after a paint");
            assert!(v.row_count() > 4, "{} rows at {cols} columns", v.row_count());
            assert_eq!(v.scroll.0, 0.0, "no horizontal scrolling while wrapping");
        });
    }
}
