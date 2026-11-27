//! Code coverage bars in the editor's gutter (see `EditorView::set_coverage_marks`).

use bf_core::coverage::LineState::{self, Covered, Partial, Uncovered};

use super::*;

/// Lines 1-6 of a method; coverage on lines 2 (covered), 3 (partly), 4 (not) and 5 (covered).
const TEXT: &str = "class A\n{\n    int M(int x)\n    {\n        if (x < 0) return -x;\n        return x;\n    }\n}\n";

fn marks() -> Vec<(usize, LineState)> {
    vec![(2, Covered), (3, Partial), (4, Uncovered), (5, Covered)]
}

/// The bars take room in the gutter (right of the line numbers) only while there is coverage to show.
#[gpui::test]
fn coverage_takes_gutter_room_while_shown(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| crate::theme::Theme::build(std::sync::Arc::new(bf_core::theme::ThemeData::default()), 0, cx).install(cx));
    let (view, cx) = cx.add_window_view(|_, cx| EditorView::new(TEXT, Some(PathBuf::from("A.cs")), cx));
    cx.run_until_parked();
    let before = view.read_with(cx, |v, _| (v.layout.gutter, v.layout.coverage));
    assert_eq!(before.1, 0.0);
    view.update(cx, |v, cx| v.set_coverage_marks(marks(), cx));
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert!(v.layout.coverage > 0.0, "room for the bars");
        assert_eq!(v.layout.gutter, before.0 + v.layout.coverage, "the gutter grows by exactly that");
    });
    view.update(cx, |v, cx| v.set_coverage_marks(Vec::new(), cx));
    cx.run_until_parked();
    view.read_with(cx, |v, _| assert_eq!((v.layout.gutter, v.layout.coverage), before, "and gives it back"));
}

/// Marks move with their lines; a line that's edited loses its mark (the run didn't measure that code).
#[gpui::test]
fn coverage_marks_follow_edits(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| crate::theme::Theme::build(std::sync::Arc::new(bf_core::theme::ThemeData::default()), 0, cx).install(cx));
    let view = cx.new(|cx| EditorView::new(TEXT, Some(PathBuf::from("A.cs")), cx));
    view.update(cx, |v, cx| {
        v.set_coverage_marks(marks(), cx);
        // A new line above the method: everything moves down one.
        v.state.set_caret(Point::new(1, 1), false);
        v.insert_text("\n", cx);
        assert_eq!(v.coverage_marks, vec![(3, Covered), (4, Partial), (5, Uncovered), (6, Covered)]);
        // A line pushed down by a line break at its start keeps its mark.
        v.state.set_caret(Point::new(5, 0), false);
        v.insert_text("\n", cx);
        assert_eq!(v.coverage_marks, vec![(3, Covered), (4, Partial), (6, Uncovered), (7, Covered)]);
        // Typing in the partly covered line: its mark goes, the others stay.
        v.state.set_caret(Point::new(4, 4), false);
        v.insert_text(" ", cx);
        assert_eq!(v.coverage_marks, vec![(3, Covered), (6, Uncovered), (7, Covered)]);
        // Deleting lines 6-7: their marks go with them.
        v.state.select(Point::new(5, 0), Point::new(7, 0));
        v.insert_text("", cx);
        assert_eq!(v.coverage_marks, vec![(3, Covered), (5, Covered)]);
    });
}
