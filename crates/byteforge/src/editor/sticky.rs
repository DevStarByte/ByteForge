//! Sticky scroll: the first lines of the blocks the top of the view is inside (namespace, class, method,
//! `#region`, an XML element) stay pinned at the top of the editor while their block scrolls by. The
//! blocks are the fold regions (found in the background), without comment runs and `using` blocks.
//! When a block's last line comes up, its pinned line is pushed up and out with it. A click on a pinned
//! line scrolls to it.

use super::*;

/// A pinned line: the buffer line and the top of its row, from the top of the editor (smaller than
/// `index * line_height` while it is being pushed out).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StickyLine {
    pub line: usize,
    pub y: f32,
}

/// Whether a region starting with `text` is a block worth pinning (not a comment run or `using` block).
pub(crate) fn is_header(text: &str) -> bool {
    let t = text.trim();
    !(t.is_empty() || t.starts_with("//") || t.starts_with("/*") || t.starts_with('*') || (t.starts_with("using ") && t.ends_with(';')) || t.starts_with("global using "))
}

/// The regions (sorted by first line) that `line` is inside of, below their first line, outermost first.
pub(crate) fn enclosing(regions: &[(usize, usize)], line: usize, mut header: impl FnMut(usize) -> bool) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for &(start, end) in regions {
        if start >= line {
            break;
        }
        if line <= end && out.last().is_none_or(|last| end <= last.1) && header(start) {
            out.push((start, end));
        }
    }
    out
}

/// The pinned lines for a view scrolled to `scroll_y`: `line_at_row` maps screen rows to lines, `row_of`
/// lines to their first row. At most `max` lines.
pub(crate) fn compute(
    regions: &[(usize, usize)],
    scroll_y: f32,
    line_height: f32,
    max: usize,
    line_at_row: impl Fn(usize) -> usize,
    row_of: impl Fn(usize) -> usize,
    mut header: impl FnMut(usize) -> bool,
) -> Vec<StickyLine> {
    if regions.is_empty() || max == 0 || line_height <= 0.0 {
        return Vec::new();
    }
    let top_row = (scroll_y / line_height).floor().max(0.0) as usize;
    // One more pinned line as long as the line that would show right under them is inside one more block.
    let mut chain: Vec<(usize, usize)> = Vec::new();
    while chain.len() < max {
        let under = line_at_row(top_row + chain.len());
        let blocks = enclosing(regions, under, &mut header);
        if blocks.len() <= chain.len() {
            break;
        }
        chain = blocks[..=chain.len()].to_vec();
    }
    chain
        .iter()
        .enumerate()
        .map(|(i, &(start, end))| {
            // The block's last line pushes its pinned line up once it reaches it.
            let end_top = row_of(end) as f32 * line_height - scroll_y;
            StickyLine { line: start, y: (i as f32 * line_height).min(end_top) }
        })
        .collect()
}

impl EditorView {
    pub(crate) fn sticky_enabled(&self) -> bool {
        self.sticky_scroll && self.sticky_max > 0
    }

    /// At most this many pinned lines: the setting, and no more than a third of the view.
    fn sticky_limit(&self) -> usize {
        let rows = (self.layout.height / self.layout.line_height.max(1.0)).floor() as usize;
        self.sticky_max.min(rows / 3)
    }

    fn is_sticky_header(&self, line: usize) -> bool {
        line < self.state.buffer.line_count() && !self.folds.is_hidden(line) && is_header(&self.state.buffer.line(line))
    }

    /// Recomputes the pinned lines for the current scroll position (every frame; cheap).
    pub(crate) fn update_sticky(&mut self) {
        if !self.sticky_enabled() {
            self.sticky.clear();
            self.sticky_hover = None;
            return;
        }
        let regions = self.folds.cached_regions();
        let sticky = compute(
            regions,
            self.scroll.1.round(),
            self.layout.line_height,
            self.sticky_limit(),
            |row| self.line_at_row(row).0,
            |line| self.row_of(line),
            |line| self.is_sticky_header(line),
        );
        if sticky.len() != self.sticky.len() {
            self.sticky_hover = None;
        }
        self.sticky = sticky;
    }

    /// The height the pinned lines cover, from the top of the editor.
    pub(crate) fn sticky_height(&self) -> f32 {
        self.sticky.iter().map(|s| s.y + self.layout.line_height).fold(0.0, f32::max)
    }

    /// Rows the pinned lines would take with `line` just below them (to keep the caret out from under them).
    pub(crate) fn sticky_rows_above(&self, line: usize) -> usize {
        if !self.sticky_enabled() {
            return 0;
        }
        enclosing(self.folds.cached_regions(), line, |l| self.is_sticky_header(l)).len().min(self.sticky_limit())
    }

    /// The pinned line under window position `(x, y)`, by index.
    pub(crate) fn sticky_at(&self, x: f32, y: f32) -> Option<usize> {
        let l = &self.layout;
        if self.sticky.is_empty() || x < l.left || x >= l.left + l.width - l.minimap || y < l.top {
            return None;
        }
        let y = y - l.top;
        // Later lines are drawn below earlier ones, so the last one whose row holds `y` is the one shown there.
        let bottom_of = |i: usize| if i == 0 { 0.0 } else { self.sticky[i - 1].y + l.line_height };
        (0..self.sticky.len()).rev().find(|&i| y >= self.sticky[i].y.max(bottom_of(i)) && y < self.sticky[i].y + l.line_height)
    }

    /// A click on a pinned line: scroll so that line shows right below the lines pinned above it, caret on it.
    pub(crate) fn click_sticky(&mut self, index: usize, x: f32, cx: &mut Context<Self>) {
        let Some(s) = self.sticky.get(index).copied() else { return };
        let lh = self.layout.line_height;
        let cell = ((x - self.layout.text_left + self.scroll.0) / self.layout.char_width.max(1.0)).max(0.0);
        let col = self.col_at_visual(s.line, cell);
        self.state.set_caret(Point::new(s.line, col), false);
        let y = (self.row_of(s.line).saturating_sub(index)) as f32 * lh;
        self.scroll_to((self.scroll_target.0, y), self.smooth_scrolling);
        self.sticky_hover = None;
        self.after_move(cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<&str> {
        text.split('\n').collect()
    }

    const CODE: &str = "using System;\nusing System.Linq;\n\nnamespace Demo\n{\n    // A comment\n    // more\n    public class A\n    {\n        void M()\n        {\n            var x = 1;\n            var y = 2;\n            var z = 3;\n        }\n\n        void N() { }\n    }\n}";

    fn compute_px(text: &str, scroll_y: f32, max: usize) -> Vec<StickyLine> {
        let ls = lines(text);
        let regions = super::super::folding::regions(&ls, super::super::folding::Style::Braces);
        compute(&regions, scroll_y, 10.0, max, |r| r.min(ls.len() - 1), |l| l, |l| is_header(ls[l]))
    }

    fn compute_at(text: &str, top: usize, max: usize) -> Vec<StickyLine> {
        compute_px(text, top as f32 * 10.0, max)
    }

    #[test]
    fn headers_skip_comments_and_usings() {
        assert!(is_header("public class A"));
        assert!(is_header("#region Helpers"));
        assert!(is_header("<PropertyGroup>"));
        assert!(!is_header("// a comment"));
        assert!(!is_header("/// <summary>"));
        assert!(!is_header("using System;"));
        assert!(!is_header("   "));
    }

    #[test]
    fn pins_the_enclosing_blocks() {
        // Top line 12 (`var y`): namespace, class and method are pinned, the line under them is line 15.
        let s = compute_at(CODE, 12, 5);
        assert_eq!(s.iter().map(|s| s.line).collect::<Vec<_>>(), vec![3, 7, 9]);
        assert_eq!(s.iter().map(|s| s.y).collect::<Vec<_>>(), vec![0.0, 10.0, 20.0]);
        // At the top of the file nothing is pinned.
        assert!(compute_at(CODE, 0, 5).is_empty());
        // The using block isn't a header.
        assert!(compute_at(CODE, 1, 5).is_empty());
    }

    #[test]
    fn deeper_blocks_come_in_as_the_view_scrolls() {
        // Top line 4 (`{` of the namespace): the namespace pinned, the line under it (5) is only inside it.
        assert_eq!(compute_at(CODE, 4, 5).iter().map(|s| s.line).collect::<Vec<_>>(), vec![3]);
        // Top line 9 (`void M()`): namespace, then class (line 10 is inside the class and M).
        assert_eq!(compute_at(CODE, 9, 5).iter().map(|s| s.line).collect::<Vec<_>>(), vec![3, 7, 9]);
        // The limit holds.
        assert_eq!(compute_at(CODE, 9, 2).iter().map(|s| s.line).collect::<Vec<_>>(), vec![3, 7]);
    }

    #[test]
    fn a_block_end_pushes_its_line_out() {
        // Half a row past line 12: M's last line (14) is at 15 px, so M's pinned line sits there too.
        let s = compute_px(CODE, 125.0, 5);
        assert_eq!(s.iter().map(|s| (s.line, s.y)).collect::<Vec<_>>(), vec![(3, 0.0), (7, 10.0), (9, 15.0)]);
        // Top line 13: M's lines are under the pinned ones; it is gone.
        assert_eq!(compute_at(CODE, 13, 5).iter().map(|s| s.line).collect::<Vec<_>>(), vec![3, 7]);
        // Top line 14: the line under two pinned lines is 16 (inside the class only).
        let s = compute_at(CODE, 14, 5);
        assert_eq!(s.iter().map(|s| s.line).collect::<Vec<_>>(), vec![3, 7]);
    }

    #[gpui::test]
    fn pinned_lines_in_the_editor(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::theme::Theme::build(std::sync::Arc::new(bf_core::theme::ThemeData::default()), 0, cx).install(cx));
        let body: String = (0..80).map(|i| format!("            var v{i} = {i};\n")).collect();
        let text = format!("namespace Demo\n{{\n    public class A\n    {{\n        void M()\n        {{\n{body}        }}\n    }}\n}}\n");
        let (view, cx) = cx.add_window_view(|_, cx| {
            let mut e = EditorView::new(&text, Some(std::path::PathBuf::from("A.cs")), cx);
            e.sticky_scroll = true;
            e
        });
        view.update_in(cx, |v, window, cx| {
            v.focus(window);
            v.refresh_fold_regions();
            cx.notify();
        });
        cx.run_until_parked();
        // Scrolled into M's body: namespace, class and method are pinned.
        view.update(cx, |v, cx| {
            let lh = v.layout.line_height;
            assert!(lh > 0.0 && v.layout.height > lh * 12.0, "laid out: {:?}", v.layout.height);
            v.scroll_to((0.0, 30.0 * lh), false);
            cx.notify();
        });
        cx.run_until_parked();
        let (x, y) = view.update(cx, |v, _| {
            assert_eq!(v.sticky.iter().map(|s| s.line).collect::<Vec<_>>(), vec![0, 2, 4]);
            let lh = v.layout.line_height;
            assert_eq!(v.sticky_height(), 3.0 * lh);
            // The class line (second pinned row) is under the pointer there.
            let (x, y) = (v.layout.text_left + 20.0, v.layout.top + lh * 1.5);
            assert_eq!(v.sticky_at(x, y), Some(1));
            assert_eq!(v.sticky_at(x, v.layout.top + lh * 3.5), None, "below the pinned lines is code");
            (x, y)
        });
        cx.simulate_mouse_move(gpui::point(px(x), px(y)), None, gpui::Modifiers::none());
        view.update(cx, |v, _| assert_eq!(v.sticky_hover, Some(1)));
        // A click goes to `public class A`, which then shows right below the namespace's pinned line.
        cx.simulate_click(gpui::point(px(x), px(y)), gpui::Modifiers::none());
        cx.run_until_parked();
        view.update(cx, |v, _| {
            assert_eq!(v.state.caret().line, 2);
            let lh = v.layout.line_height;
            assert_eq!(v.scroll_target.1, lh, "line 2 on the second row");
        });
        // The caret going up stays below the pinned lines: Up from line 30 at the top scrolls.
        view.update(cx, |v, _| {
            let lh = v.layout.line_height;
            v.scroll_to((0.0, 28.0 * lh), false);
            v.state.set_caret(Point::new(30, 0), false);
            v.ensure_caret_visible();
            assert_eq!(v.scroll_target.1, 27.0 * lh, "line 30 right below three pinned lines");
        });
    }

    #[test]
    fn nested_only() {
        // Overlapping (not nested) regions after the first aren't stacked.
        let regions = vec![(0, 10), (2, 12), (3, 5)];
        let e = enclosing(&regions, 4, |_| true);
        assert_eq!(e, vec![(0, 10), (3, 5)]);
    }
}
