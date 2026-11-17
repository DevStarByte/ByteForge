//! Go Back / Go Forward (Alt+Left / Alt+Right, the mouse's back and forward buttons): a history of
//! where the caret was, like a browser's. A place is remembered when you leave it for another file, or
//! jump more than a few lines within one (go to definition, Ctrl+G, a click further away); typing and
//! small moves only update the current place, so Back returns to where the caret last was.

use super::*;

/// Moves within this many lines count as staying in the same place.
const NEAR_LINES: usize = 10;
/// The most places kept in each direction.
const MAX_PLACES: usize = 50;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Place {
    pub path: PathBuf,
    pub at: Point,
}

impl Place {
    fn near(&self, other: &Place) -> bool {
        bf_core::paths::same_path(&self.path, &other.path) && self.at.line.abs_diff(other.at.line) <= NEAR_LINES
    }
}

#[derive(Default)]
pub(super) struct NavHistory {
    back: Vec<Place>,
    forward: Vec<Place>,
    /// Where the caret is now (as far as the history knows).
    current: Option<Place>,
}

impl NavHistory {
    /// The caret is at `place`: a jump away from the current place remembers that one (and drops the
    /// places ahead), a small move just follows the caret.
    pub fn record(&mut self, place: Place) {
        match self.current.take() {
            Some(current) if !current.near(&place) => {
                self.back.push(current);
                if self.back.len() > MAX_PLACES {
                    self.back.remove(0);
                }
                self.forward.clear();
            }
            _ => {}
        }
        self.current = Some(place);
    }

    /// The place to go back to; the current one becomes the first place ahead.
    pub fn back(&mut self) -> Option<Place> {
        let place = self.back.pop()?;
        if let Some(current) = self.current.replace(place.clone()) {
            self.forward.push(current);
        }
        Some(place)
    }

    /// The place to go forward to; the current one becomes the last place behind.
    pub fn forward(&mut self) -> Option<Place> {
        let place = self.forward.pop()?;
        if let Some(current) = self.current.replace(place.clone()) {
            self.back.push(current);
        }
        Some(place)
    }

    /// Forgets a place that can't be opened any more.
    fn drop_current(&mut self) {
        self.current = None;
    }
}

impl Workbench {
    /// Where the caret of the focused file tab is.
    fn active_place(&self, cx: &App) -> Option<Place> {
        let tab = self.active.and_then(|i| self.tabs.get(i)).filter(|t| t.page.is_none())?;
        Some(Place { path: tab.path.clone()?, at: tab.editor.read(cx).state.caret() })
    }

    /// An editor's caret moved: remembered when it is the focused tab's (moves in other tabs, e.g. while
    /// a session is restored or a file view syncs, aren't places you were).
    pub(super) fn record_place(&mut self, editor: &Entity<EditorView>, cx: &App) {
        if self.active.and_then(|i| self.tabs.get(i)).is_some_and(|t| t.editor == *editor)
            && let Some(place) = self.active_place(cx)
        {
            self.nav.record(place);
        }
    }

    pub(super) fn go_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.navigate(true, window, cx);
    }

    pub(super) fn go_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.navigate(false, window, cx);
    }

    fn navigate(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        // Switching tabs doesn't move a caret: count the tab in front as where you are.
        if let Some(place) = self.active_place(cx) {
            self.nav.record(place);
        }
        loop {
            let next = if back { self.nav.back() } else { self.nav.forward() };
            let Some(place) = next else { return };
            if place.path.is_file() {
                self.open_file(place.path, Some(place.at), window, cx);
                return;
            }
            // The file is gone: skip it.
            self.nav.drop_current();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(path: &str, line: usize) -> Place {
        Place { path: PathBuf::from(path), at: Point { line, col: 0 } }
    }

    #[test]
    fn small_moves_follow_the_caret_and_jumps_are_remembered() {
        let mut nav = NavHistory::default();
        nav.record(at("/a.cs", 10));
        nav.record(at("/a.cs", 12)); // typing, small moves
        nav.record(at("/a.cs", 80)); // a jump in the file
        nav.record(at("/b.cs", 5)); // another file
        nav.record(at("/b.cs", 6));
        assert_eq!(nav.back(), Some(at("/a.cs", 80)));
        assert_eq!(nav.back(), Some(at("/a.cs", 12)), "back to where the caret last was, not where it arrived");
        assert_eq!(nav.back(), None);
        // Forward returns to where Back was pressed.
        assert_eq!(nav.forward(), Some(at("/a.cs", 80)));
        assert_eq!(nav.forward(), Some(at("/b.cs", 6)));
        assert_eq!(nav.forward(), None);
    }

    #[test]
    fn a_new_jump_after_going_back_drops_the_places_ahead() {
        let mut nav = NavHistory::default();
        nav.record(at("/a.cs", 1));
        nav.record(at("/b.cs", 1));
        assert_eq!(nav.back(), Some(at("/a.cs", 1)));
        nav.record(at("/a.cs", 3)); // arriving there: a small move, the place ahead stays
        assert_eq!(nav.forward(), Some(at("/b.cs", 1)));
        nav.back();
        nav.record(at("/c.cs", 1)); // a new jump
        assert_eq!(nav.forward(), None);
        assert_eq!(nav.back(), Some(at("/a.cs", 3)));
    }

    /// The real workbench: go to another file, jump in it, then Back and Forward retrace the carets.
    #[gpui::test]
    fn back_and_forward_in_the_workbench(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = crate::workbench::settings_page::view_tests::workbench(cx, "navigation");
        std::fs::create_dir_all(guard.dir.join("proj")).unwrap();
        let root = normalize_path(&guard.dir.join("proj"));
        let (a, b) = (root.join("a.txt"), root.join("b.txt"));
        std::fs::write(&a, "line\n".repeat(100)).unwrap();
        std::fs::write(&b, "line\n".repeat(100)).unwrap();
        let caret = |wb: &Workbench, cx: &App| wb.active_place(cx).unwrap();
        wb.update_in(cx, |wb, window, cx| {
            wb.open_folder(root.clone(), window, cx);
            wb.open_file(a.clone(), Some(Point { line: 20, col: 2 }), window, cx);
        });
        cx.run_until_parked();
        wb.update_in(cx, |wb, window, cx| wb.open_file(b.clone(), Some(Point { line: 3, col: 0 }), window, cx));
        cx.run_until_parked();
        wb.update_in(cx, |wb, window, cx| wb.open_file(b.clone(), Some(Point { line: 70, col: 1 }), window, cx));
        cx.run_until_parked();
        wb.update_in(cx, |wb, window, cx| wb.go_back(window, cx));
        cx.run_until_parked();
        wb.update_in(cx, |wb, _, cx| assert_eq!(caret(wb, cx), Place { path: b.clone(), at: Point { line: 3, col: 0 } }));
        wb.update_in(cx, |wb, window, cx| wb.go_back(window, cx));
        cx.run_until_parked();
        wb.update_in(cx, |wb, _, cx| assert_eq!(caret(wb, cx), Place { path: a.clone(), at: Point { line: 20, col: 2 } }));
        wb.update_in(cx, |wb, window, cx| wb.go_forward(window, cx));
        cx.run_until_parked();
        wb.update_in(cx, |wb, window, cx| wb.go_forward(window, cx));
        cx.run_until_parked();
        wb.update_in(cx, |wb, _, cx| assert_eq!(caret(wb, cx), Place { path: b.clone(), at: Point { line: 70, col: 1 } }));
        drop(guard);
    }
}
