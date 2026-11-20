//! Code Lens: a row of small text above declarations, like VS Code's and Rider's: how often a type or member
//! is referenced (`textDocument/codeLens`, resolved lazily), and "Run Test | Debug Test" above tests (found in
//! the source, as the Test Explorer finds them). Clicking an entry lists the references or runs the test.
//!
//! The row is a spacer row (see `wrap.rs`): the declaration's text starts one row further down, and a lens
//! row folds away with its line. It sits above the declaration's attributes (`[Fact]`), as in Rider.

use std::sync::Arc;
use std::time::Duration;

use bf_text::Point;
use gpui::Context;

use super::{EditorEvent, EditorView};

/// What clicking a lens entry does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LensAction {
    /// Find All References for the symbol at this position.
    References(Point),
    /// Run (or debug) the test or test class declared on this line.
    RunTest { line: usize, debug: bool },
    /// Run the benchmark (or every benchmark of the class) declared on this line.
    RunBenchmark { line: usize },
}

/// Something in the file a lens runs: a test or a BenchmarkDotNet benchmark (`class`: all of a class's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Runnable {
    Test { class: bool },
    Benchmark { class: bool },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LensItem {
    /// Empty while the server hasn't resolved it (the row is kept, so the text doesn't jump).
    pub title: String,
    pub action: LensAction,
}

/// The entries on the lens row above `line`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LensRow {
    pub line: usize,
    pub items: Vec<LensItem>,
}

/// Separates the entries of one row.
pub(crate) const SEPARATOR: &str = "  |  ";

impl LensRow {
    /// The row's text and, for each entry with a title, its byte range in that text.
    pub(crate) fn label(&self) -> (String, Vec<(usize, usize, &LensAction)>) {
        let mut text = String::new();
        let mut spans = Vec::new();
        for item in self.items.iter().filter(|i| !i.title.is_empty()) {
            if !text.is_empty() {
                text.push_str(SEPARATOR);
            }
            spans.push((text.len(), text.len() + item.title.len(), &item.action));
            text.push_str(&item.title);
        }
        (text, spans)
    }
}

/// "3 Reference(s)" (csharp-ls) → "3 references", "1 Reference(s)" → "1 reference"; other titles stay.
pub(crate) fn tidy_title(title: &str) -> String {
    let title = title.trim();
    if let Some(n) = title.strip_suffix(" Reference(s)").and_then(|n| n.trim().parse::<u64>().ok()) {
        return if n == 1 { "1 reference".to_owned() } else { format!("{n} references") };
    }
    title.to_owned()
}

/// The line a lens row goes above: the declaration's first attribute line (`[Fact]`, `[HttpGet("…")]`), or the
/// declaration itself.
pub(crate) fn anchor_line(lines: &[&str], decl: usize) -> usize {
    let mut line = decl.min(lines.len().saturating_sub(1));
    while line > 0 && lines[line - 1].trim_start().starts_with('[') {
        line -= 1;
    }
    line
}

/// The rows for `lenses` (server lenses, their declaration line and title so far) and the tests and benchmarks
/// declared in the file, one row per anchor line, references first.
pub(crate) fn build_rows(lines: &[&str], lenses: &[(Point, String)], runnables: &[(usize, Runnable)]) -> Vec<LensRow> {
    let mut rows: Vec<LensRow> = Vec::new();
    let push = |rows: &mut Vec<LensRow>, decl: usize, item: LensItem| {
        let line = anchor_line(lines, decl);
        match rows.iter_mut().find(|r| r.line == line) {
            Some(row) => row.items.push(item),
            None => rows.push(LensRow { line, items: vec![item] }),
        }
    };
    for (at, title) in lenses {
        push(&mut rows, at.line, LensItem { title: title.clone(), action: LensAction::References(*at) });
    }
    for &(line, runnable) in runnables {
        match runnable {
            Runnable::Test { class } => {
                let (run, debug) = if class { ("Run All Tests", "Debug All Tests") } else { ("Run Test", "Debug Test") };
                push(&mut rows, line, LensItem { title: run.into(), action: LensAction::RunTest { line, debug: false } });
                push(&mut rows, line, LensItem { title: debug.into(), action: LensAction::RunTest { line, debug: true } });
            }
            Runnable::Benchmark { class } => {
                let title = if class { "Run All Benchmarks" } else { "Run Benchmark" };
                push(&mut rows, line, LensItem { title: title.into(), action: LensAction::RunBenchmark { line } });
            }
        }
    }
    rows.sort_by_key(|r| r.line);
    rows
}

/// Lenses resolved per round trip while filling in titles (the visible ones go first).
const RESOLVE_BATCH: usize = 8;

impl EditorView {
    /// Reads the file's lenses again after `delay` (edits restart the wait), shows their rows, then resolves
    /// their titles, the visible ones first.
    pub(crate) fn schedule_code_lenses(&mut self, delay: Duration, cx: &mut Context<Self>) {
        // The diff editor's spacer rows would fight with lens rows; it shows none.
        if !self.code_lens_enabled || self.spacers.is_some() {
            if !self.lens_rows.is_empty() {
                self.lens_rows.clear();
                self.lens_hits.clear();
                self.lens_hover = None;
                self.rebuild_lens_spacers();
                cx.notify();
            }
            self.lens_task = None;
            return;
        }
        let (Some(lsp), Some(path)) = (self.lsp.clone(), self.path.clone()) else { return };
        let version = self.state.buffer.version();
        self.lens_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let mut lenses = match lsp.code_lenses(&path).recv_async().await {
                Ok(Ok(lenses)) => lenses,
                // A server without lenses: the test lenses still show.
                Ok(Err(_)) => Vec::new(),
                Err(_) => return,
            };
            // Roslyn's own test lenses run through VS Code's test explorer; ByteForge adds its own.
            lenses.retain(|l| !l.command.as_deref().is_some_and(|c| c.starts_with("dotnet.test")));
            let tests = {
                let path = path.clone();
                let Ok(text) = this.update(cx, |view, _| (view.state.buffer.version() == version).then(|| view.state.buffer.text())) else { return };
                let Some(text) = text else { return };
                cx.background_executor()
                    .spawn(async move {
                        let mut found = Vec::new();
                        for class in bf_core::testing::scan_source(&text, &path) {
                            found.push((class.line, Runnable::Test { class: true }));
                            found.extend(class.tests.iter().map(|t| (t.line, Runnable::Test { class: false })));
                        }
                        for class in bf_core::benchmarks::scan_source(&text) {
                            found.push((class.line, Runnable::Benchmark { class: true }));
                            found.extend(class.methods.iter().map(|(_, line)| (*line, Runnable::Benchmark { class: false })));
                        }
                        found
                    })
                    .await
            };
            let Ok(Some(order)) = this.update(cx, |view, cx| {
                if view.state.buffer.version() != version {
                    return None;
                }
                view.set_lenses(&lenses, &tests, cx);
                let (first, last) = view.visible_lines();
                let mut pending: Vec<usize> = (0..lenses.len()).filter(|&i| lenses[i].needs_resolve()).collect();
                pending.sort_by_key(|&i| {
                    let line = lenses[i].range.start.line as usize;
                    if (first..=last).contains(&line) { 0 } else { line.abs_diff((first + last) / 2) }
                });
                Some(pending)
            }) else {
                return;
            };
            for batch in order.chunks(RESOLVE_BATCH) {
                let answers: Vec<_> = batch.iter().map(|&i| (i, lsp.resolve_code_lens(&lenses[i]))).collect();
                for (i, rx) in answers {
                    if let Ok(Ok(resolved)) = rx.recv_async().await {
                        lenses[i] = resolved;
                    }
                }
                let current = this.update(cx, |view, cx| {
                    let current = view.state.buffer.version() == version;
                    if current {
                        view.set_lenses(&lenses, &tests, cx);
                    }
                    current
                });
                if !matches!(current, Ok(true)) {
                    return;
                }
            }
        }));
    }

    /// Shows `lenses` (server, unresolved ones keep the title their row had) and `tests`.
    fn set_lenses(&mut self, lenses: &[bf_lsp::CodeLens], runnables: &[(usize, Runnable)], cx: &mut Context<Self>) {
        self.has_benchmarks = runnables.iter().any(|(_, r)| matches!(r, Runnable::Benchmark { .. }));
        let buffer = &self.state.buffer;
        let found: Vec<(Point, String)> = lenses
            .iter()
            .map(|l| {
                let at = buffer.utf16_to_point(l.range.start.line as usize, l.range.start.character as usize);
                let title = match &l.title {
                    Some(title) => tidy_title(title),
                    // Until it is resolved, the count the line had before (no flicker while typing).
                    None => self
                        .lens_rows
                        .iter()
                        .flat_map(|r| &r.items)
                        .find(|i| matches!(i.action, LensAction::References(p) if p.line == at.line))
                        .map(|i| i.title.clone())
                        .unwrap_or_default(),
                };
                (at, title)
            })
            .collect();
        let text = buffer.text();
        let lines: Vec<&str> = text.lines().collect();
        let rows = build_rows(&lines, &found, runnables);
        if rows != self.lens_rows {
            self.lens_rows = rows;
            self.rebuild_lens_spacers();
            cx.notify();
        }
    }

    /// One spacer row above each line with lenses (for the row map in `wrap.rs`).
    pub(crate) fn rebuild_lens_spacers(&mut self) {
        if self.lens_rows.is_empty() {
            self.lens_spacers = None;
            return;
        }
        let lines = self.state.buffer.line_count();
        let mut spacers = vec![0; lines + 1];
        for row in &self.lens_rows {
            if row.line < lines {
                spacers[row.line] = 1;
            }
        }
        self.lens_spacers = Some(Arc::new(spacers));
    }

    /// Keeps the rows on their declarations while edits happen (until the server's next answer).
    pub(crate) fn shift_lenses(&mut self, c: &bf_text::Change) {
        if self.lens_rows.is_empty() {
            return;
        }
        let shift = |line: usize| super::inlays::shift_point(Point::new(line, 0), c).line;
        for row in &mut self.lens_rows {
            row.line = shift(row.line);
            for item in &mut row.items {
                match &mut item.action {
                    LensAction::References(p) => *p = super::inlays::shift_point(*p, c),
                    LensAction::RunTest { line, .. } | LensAction::RunBenchmark { line } => *line = shift(*line),
                }
            }
        }
        // Deleted lines can bring two rows onto one line: keep the first.
        self.lens_rows.dedup_by_key(|r| r.line);
        self.lens_hover = None;
        self.rebuild_lens_spacers();
    }

    /// The lens entry under the window position `(x, y)`.
    pub(crate) fn lens_at(&self, x: f32, y: f32) -> Option<(usize, usize, LensAction)> {
        let l = &self.layout;
        if self.lens_hits.is_empty() || x < l.text_left || y < l.top || y >= l.top + l.height {
            return None;
        }
        let row = (((y - l.top) + self.scroll.1) / l.line_height.max(1.0)).floor().max(0.0) as usize;
        let offset = x - (l.text_left - self.scroll.0);
        self.lens_hits
            .iter()
            .enumerate()
            .find(|(_, h)| !self.folds.is_hidden(h.line) && self.row_of(h.line).checked_sub(1) == Some(row) && h.x0 <= offset && offset < h.x1)
            .map(|(i, h)| (h.line, i, h.action.clone()))
    }

    /// Runs a clicked lens entry.
    pub(crate) fn run_lens(&mut self, action: LensAction, cx: &mut Context<Self>) {
        match action {
            LensAction::References(at) => self.find_references_at(at, cx),
            LensAction::RunTest { line, debug } => cx.emit(EditorEvent::RunTestAt { line, debug }),
            LensAction::RunBenchmark { line } => cx.emit(EditorEvent::RunBenchmarkAt { line }),
        }
    }

    /// Asks for the lenses again soon (other files changed what they count). Rows stay until the answer.
    pub fn refresh_code_lenses(&mut self, cx: &mut Context<Self>) {
        if self.lsp.is_some() && self.code_lens_enabled {
            self.schedule_code_lenses(Duration::from_millis(300), cx);
        }
    }

    /// Turns Code Lens on or off (`"editor.codeLens"`).
    pub fn set_code_lens(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.code_lens_enabled != enabled {
            self.code_lens_enabled = enabled;
            self.schedule_code_lenses(Duration::ZERO, cx);
        }
    }
}

/// Where a lens entry was drawn, relative to the text's left edge (scrolled), so a click on it runs it.
#[derive(Clone, Debug)]
pub(crate) struct LensHit {
    pub line: usize,
    pub x0: f32,
    pub x1: f32,
    pub action: LensAction,
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    #[test]
    fn titles_are_tidied() {
        assert_eq!(tidy_title("0 Reference(s)"), "0 references");
        assert_eq!(tidy_title("1 Reference(s)"), "1 reference");
        assert_eq!(tidy_title("12 Reference(s)"), "12 references");
        assert_eq!(tidy_title("3 references"), "3 references");
    }

    #[test]
    fn benchmarks_get_a_run_lens_above_their_attributes() {
        let text = "public class Sorts\n{\n    [Benchmark]\n    public void ArraySort() { }\n}";
        let lines: Vec<&str> = text.lines().collect();
        let rows = build_rows(&lines, &[], &[(0, Runnable::Benchmark { class: true }), (3, Runnable::Benchmark { class: false })]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].label().0, "Run All Benchmarks");
        assert_eq!((rows[1].line, rows[1].label().0.as_str()), (2, "Run Benchmark"), "above [Benchmark]");
        assert_eq!(rows[1].items[0].action, LensAction::RunBenchmark { line: 3 });
    }

    #[test]
    fn rows_go_above_attributes_and_join_tests() {
        let text = "class CalcTests\n{\n    [Fact]\n    [Trait(\"a\", \"b\")]\n    public void Adds() { }\n}";
        let lines: Vec<&str> = text.lines().collect();
        let lenses = vec![(Point::new(0, 6), "2 references".to_owned()), (Point::new(4, 16), "0 references".to_owned())];
        let rows = build_rows(&lines, &lenses, &[(0, Runnable::Test { class: true }), (4, Runnable::Test { class: false })]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].line, 0);
        assert_eq!(rows[0].label().0, "2 references  |  Run All Tests  |  Debug All Tests");
        // Above `[Fact]`, not between the attributes and the method.
        assert_eq!(rows[1].line, 2);
        let (text, spans) = rows[1].label();
        assert_eq!(text, "0 references  |  Run Test  |  Debug Test");
        assert_eq!(spans.len(), 3);
        assert_eq!(&text[spans[2].0..spans[2].1], "Debug Test");
        assert_eq!(spans[2].2, &LensAction::RunTest { line: 4, debug: true });
        assert_eq!(spans[0].2, &LensAction::References(Point::new(4, 16)));
    }

    #[gpui::test]
    fn lens_rows_take_a_row_and_run_when_clicked(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::theme::Theme::build(Arc::new(bf_core::theme::ThemeData::default()), 0, cx).install(cx));
        let text = "public class CalcTests\n{\n    [Fact]\n    public void Adds() { }\n}\n";
        let (view, cx) = cx.add_window_view(|_, cx| EditorView::new(text, None, cx));
        view.update(cx, |v, _| {
            let lines: Vec<&str> = text.lines().collect();
            v.lens_rows = build_rows(&lines, &[(Point::new(3, 16), "1 reference".into())], &[(3, Runnable::Test { class: false })]);
            v.rebuild_lens_spacers();
            v.update_wrap();
            // The row goes above `[Fact]` (line 2): lines 0 and 1, the lens row, then line 2.
            assert_eq!(v.row_of(1), 1);
            assert_eq!(v.row_of(2), 3);
            assert_eq!(v.row_count(), 6 + 1);
        });
        let events = Rc::new(RefCell::new(Vec::new()));
        let seen = events.clone();
        cx.update(|_, cx| {
            cx.subscribe(&view, move |_, e: &EditorEvent, _| {
                if let EditorEvent::RunTestAt { line, debug } = e {
                    seen.borrow_mut().push((*line, *debug));
                }
            })
            .detach()
        });
        view.update_in(cx, |v, window, cx| {
            v.focus(window);
            cx.notify();
        });
        cx.run_until_parked();
        view.update(cx, |v, cx| {
            let labels: Vec<_> = v.lens_hits.iter().map(|h| h.action.clone()).collect();
            assert_eq!(labels.len(), 3, "references, Run Test, Debug Test drawn: {labels:?}");
            let l = v.layout;
            let hit = v.lens_hits[2].clone();
            let x = l.text_left - v.scroll.0 + (hit.x0 + hit.x1) / 2.0;
            let y = l.top + (v.row_of(2) - 1) as f32 * l.line_height - v.scroll.1 + l.line_height / 2.0;
            let (line, _, action) = v.lens_at(x, y).expect("Debug Test under the pointer");
            assert_eq!(line, 2);
            assert!(v.lens_at(x, y + l.line_height).is_none(), "the [Fact] line itself isn't a lens");
            v.run_lens(action, cx);

            // Typing a line above moves the row with its declaration.
            v.state.set_caret(Point::new(0, 0), false);
            v.edit(cx, |s| s.type_text("using Xunit;\n"));
            assert_eq!(v.lens_rows[0].line, 3);
            assert_eq!(v.lens_rows[0].items[2].action, LensAction::RunTest { line: 4, debug: true });
        });
        assert_eq!(*events.borrow(), [(3, true)]);
    }

    #[test]
    fn unresolved_entries_take_no_space() {
        let row = LensRow { line: 0, items: vec![LensItem { title: String::new(), action: LensAction::References(Point::new(0, 0)) }] };
        assert_eq!(row.label().0, "");
    }
}
