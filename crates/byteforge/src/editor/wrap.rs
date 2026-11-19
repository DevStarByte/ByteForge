//! Word wrap: long lines continue on the next screen rows instead of running off to the right.
//!
//! The editor font is monospaced, so wrapping is column arithmetic: with `cols` columns fitting in the
//! text area, a line whose visual length (tabs expanded, inlay hints included) is `n` takes
//! `max(1, ceil(n / cols))` rows, and visual column `v` sits on row `v / cols` of its line at
//! `v % cols`. The [`WrapMap`] keeps the first row of every line; everything that turns positions into
//! screen coordinates (painting, clicks, the caret, scrolling, Up/Down) goes through it. Folded lines
//! take no rows.
//!
//! The same map places spacer rows (empty rows that aren't text, see `EditorView::spacers`), with or
//! without wrapping: they sit above their line, so its text starts that many rows further down.

use std::sync::Arc;

use super::*;

/// The screen row each line starts on, for one text, width, set of folds, inlay hints and spacers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WrapMap {
    /// Columns per row while wrapping; `None` when only spacers need the map.
    pub cols: Option<usize>,
    /// `starts[l]` is the first row of line `l`'s text; `starts[line_count]` is the number of rows.
    pub starts: Arc<Vec<usize>>,
    /// The spacer rows above each line as laid out (none above folded lines); `line_count + 1` long, or
    /// empty without spacers.
    pub spacers: Arc<Vec<usize>>,
    key: (u64, usize, u64, usize),
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

/// The first rows of lines whose text takes `rows[l]` rows each, with `spacers[l]` rows above line `l`
/// (`spacers[line_count]` below the last line; missing entries are 0).
pub(crate) fn starts_from(rows: impl Iterator<Item = usize>, spacers: &[usize]) -> Vec<usize> {
    let sp = |l: usize| spacers.get(l).copied().unwrap_or(0);
    let mut acc = sp(0);
    let mut starts = vec![acc];
    for (l, r) in rows.enumerate() {
        acc += r + sp(l + 1);
        starts.push(acc);
    }
    starts
}

/// Rows of `line`'s own text (without the spacers below it).
pub(crate) fn text_rows(starts: &[usize], spacers: &[usize], line: usize) -> usize {
    let next = spacers.get(line + 1).copied().unwrap_or(0);
    (starts[line + 1] - next).saturating_sub(starts[line])
}

/// The line and segment shown on `row`. A spacer row belongs to the last row of the line above it
/// (the first line, for spacers above it).
pub(crate) fn line_at(starts: &[usize], spacers: &[usize], row: usize) -> (usize, usize) {
    let lines = starts.len().saturating_sub(1);
    if lines == 0 {
        return (0, 0);
    }
    // The last line whose first row is at or before `row` (and that has rows: folded lines have none).
    let mut line = starts.partition_point(|&s| s <= row).saturating_sub(1).min(lines - 1);
    while line > 0 && text_rows(starts, spacers, line) == 0 {
        line -= 1;
    }
    let seg = row.saturating_sub(starts[line]).min(text_rows(starts, spacers, line).saturating_sub(1));
    (line, seg)
}

impl EditorView {
    /// Columns per row while wrapping (`None` when word wrap is off or the size isn't known yet).
    pub(crate) fn wrap_cols(&self) -> Option<usize> {
        self.wrap.as_ref().and_then(|w| w.cols)
    }

    /// A line's width in columns: tabs expanded, inlay hints included.
    pub(crate) fn visual_len(&self, line: usize) -> usize {
        let text = self.state.buffer.line(line);
        let len = text.chars().count();
        bf_text::visual_col(&text, len, self.state.tab_size) + self.inlay_shift(line, len, false)
    }

    /// Rebuilds the row map when wrapping (or showing spacers) and the text, width, folds, inlay hints or
    /// spacers changed (every frame).
    pub(crate) fn update_wrap(&mut self) {
        let wrapping = self.word_wrap && self.layout.char_width > 0.0 && self.layout.width > 0.0;
        let spacers = self.spacers.clone().or_else(|| self.lens_spacers.clone()).filter(|s| s.iter().any(|&n| n > 0));
        if !wrapping && spacers.is_none() {
            self.wrap = None;
            return;
        }
        // One column of slack so the caret at the end of a full row stays visible.
        let cols = wrapping.then(|| ((self.layout.text_width() / self.layout.char_width).floor() as usize).saturating_sub(1).max(10));
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(&self.folds.hidden_ranges(), &mut hasher);
        std::hash::Hash::hash(&self.inlays, &mut hasher);
        let spacers_id = spacers.as_ref().map_or(0, |s| Arc::as_ptr(s) as usize);
        let key = (self.state.buffer.version(), cols.unwrap_or(0), std::hash::Hasher::finish(&hasher), spacers_id);
        if self.wrap.as_ref().is_some_and(|w| w.key == key) {
            return;
        }
        let lines = self.state.buffer.line_count();
        // Spacers above a folded line fold away with it.
        let laid_out: Vec<usize> = match &spacers {
            Some(s) => (0..=lines).map(|l| if l < lines && self.folds.is_hidden(l) { 0 } else { s.get(l).copied().unwrap_or(0) }).collect(),
            None => Vec::new(),
        };
        let rows = (0..lines).map(|l| match cols {
            _ if self.folds.is_hidden(l) => 0,
            Some(cols) => rows_for(self.visual_len(l), cols),
            None => 1,
        });
        let starts = starts_from(rows, &laid_out);
        self.wrap = Some(WrapMap { cols, starts: Arc::new(starts), spacers: Arc::new(laid_out), key });
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
            Some(w) => line_at(&w.starts, &w.spacers, row),
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
            None => (self.row_of(p.line), v),
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
        if self.wrap_cols().is_some() && !self.state.has_multiple_cursors() {
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
        let starts = starts_from([1, 3, 0, 2].into_iter(), &[]);
        assert_eq!(starts, [0, 1, 4, 4, 6]);
        assert_eq!(line_at(&starts, &[], 0), (0, 0));
        assert_eq!(line_at(&starts, &[], 2), (1, 1));
        assert_eq!(line_at(&starts, &[], 3), (1, 2));
        assert_eq!(line_at(&starts, &[], 4), (3, 0));
        assert_eq!(line_at(&starts, &[], 5), (3, 1));
        assert_eq!(line_at(&starts, &[], 99), (3, 1));
    }

    #[test]
    fn spacer_rows_push_lines_down() {
        // One spacer above line 0, two above line 2, one below the last (line 2).
        let spacers = [1, 0, 2, 1];
        let starts = starts_from([1, 1, 1].into_iter(), &spacers);
        assert_eq!(starts, [1, 2, 5, 7]);
        assert_eq!(text_rows(&starts, &spacers, 1), 1);
        assert_eq!(line_at(&starts, &spacers, 0), (0, 0), "a spacer above the first line belongs to it");
        assert_eq!(line_at(&starts, &spacers, 3), (1, 0), "a spacer row belongs to the line above");
        assert_eq!(line_at(&starts, &spacers, 5), (2, 0));
        assert_eq!(line_at(&starts, &spacers, 6), (2, 0));
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
            v.layout = Layout { left: 0.0, top: 0.0, width: 240.0, height: 400.0, gutter: 40.0, text_left: 40.0, char_width: 10.0, line_height: 20.0, glyph: 0.0, fold_left: 0.0, fold_width: 0.0, minimap: 0.0, blame: 0.0 };
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
