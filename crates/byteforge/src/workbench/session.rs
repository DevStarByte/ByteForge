//! The last session, restored on the next start: the open files of each folder (their editor group,
//! caret, and which tab was in front), and the window's position and size.

use gpui::{Bounds, WindowBounds};
use serde::{Deserialize, Serialize};

use super::*;

#[derive(Serialize, Deserialize, Default, Debug, PartialEq)]
pub(super) struct SavedSession {
    tabs: Vec<SavedTab>,
    /// Indexes into `tabs`: the focused tab, and the tab in front in each editor group.
    active: Option<usize>,
    visible: [Option<usize>; 2],
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct SavedTab {
    path: String,
    group: usize,
    line: usize,
    col: usize,
}

/// The window's place: `Windowed`, `Maximized` or `Fullscreen` with its normal (restored) bounds.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct SavedWindow {
    pub mode: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

pub const WINDOW_KEY: &str = "window";

impl SavedWindow {
    pub fn from_bounds(bounds: WindowBounds) -> Self {
        let (mode, b) = match bounds {
            WindowBounds::Windowed(b) => ("windowed", b),
            WindowBounds::Maximized(b) => ("maximized", b),
            WindowBounds::Fullscreen(b) => ("fullscreen", b),
        };
        Self {
            mode: mode.into(),
            x: f32::from(b.origin.x),
            y: f32::from(b.origin.y),
            width: f32::from(b.size.width),
            height: f32::from(b.size.height),
        }
    }

    /// The saved bounds, when they are sane and still on a connected display.
    pub fn to_bounds(&self, cx: &App) -> Option<WindowBounds> {
        if !(self.width >= 200.0 && self.height >= 150.0 && self.x.is_finite() && self.y.is_finite()) {
            return None;
        }
        let b = Bounds::new(gpui::point(px(self.x), px(self.y)), gpui::size(px(self.width), px(self.height)));
        // At least a title bar's worth of the window must be on some screen.
        let grip = Bounds::new(b.origin, gpui::size(b.size.width, px(40.)));
        if !cx.displays().iter().any(|d| d.bounds().intersects(&grip)) {
            return None;
        }
        Some(match self.mode.as_str() {
            "maximized" => WindowBounds::Maximized(b),
            "fullscreen" => WindowBounds::Fullscreen(b),
            _ => WindowBounds::Windowed(b),
        })
    }
}

impl Workbench {
    fn session_key(&self) -> Option<String> {
        self.workspace.root().map(|root| format!("session:{}", root.display()))
    }

    /// The open file tabs (pages such as previews and diffs are left out).
    fn current_session(&self, cx: &App) -> SavedSession {
        let mut session = SavedSession::default();
        let mut index = vec![None; self.tabs.len()];
        for (i, tab) in self.tabs.iter().enumerate() {
            let Some(path) = tab.path.as_ref().filter(|_| tab.page.is_none()) else { continue };
            let caret = tab.editor.read(cx).state.caret();
            index[i] = Some(session.tabs.len());
            session.tabs.push(SavedTab { path: path.to_string_lossy().into_owned(), group: tab.group, line: caret.line, col: caret.col });
        }
        let map = |ix: Option<usize>| ix.and_then(|i| index.get(i).copied().flatten());
        session.active = map(self.active);
        session.visible = [map(self.visible[0]), map(self.visible[1])];
        session
    }

    /// Remembers the open files of the current folder, and the window's place.
    pub(super) fn save_session(&self, window: Option<&Window>, cx: &App) {
        if let Some(key) = self.session_key() {
            let session = self.current_session(cx);
            let text = serde_json::to_string(&session).unwrap_or_default();
            self.state.set(&key, (!session.tabs.is_empty()).then_some(text.as_str()));
        }
        if let Some(window) = window {
            let saved = SavedWindow::from_bounds(window.window_bounds());
            self.state.set(WINDOW_KEY, serde_json::to_string(&saved).ok().as_deref());
        }
    }

    /// Opens the files that were open when the current folder was last closed, each with its caret.
    pub(super) fn restore_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.session_key() else { return };
        let Some(session) = self.state.get(&key).and_then(|t| serde_json::from_str::<SavedSession>(&t).ok()) else { return };
        let mut opened: Vec<Option<usize>> = Vec::with_capacity(session.tabs.len());
        for saved in &session.tabs {
            let path = PathBuf::from(&saved.path);
            let already = self.tabs.iter().any(|t| t.path.as_deref().is_some_and(|p| bf_core::paths::same_path(p, &path)));
            if already || !path.is_file() {
                opened.push(None);
                continue;
            }
            self.focus_group = saved.group.min(1);
            let before = self.tabs.len();
            self.open_file(path, None, window, cx);
            if self.tabs.len() == before {
                opened.push(None);
                continue;
            }
            let ix = self.tabs.len() - 1;
            self.tabs[ix].editor.update(cx, |e, cx| {
                let at = e.state.buffer.clamp(Point { line: saved.line, col: saved.col });
                e.go_to(at, cx);
            });
            opened.push(Some(ix));
        }
        let map = |ix: Option<usize>| ix.and_then(|i| opened.get(i).copied().flatten());
        self.normalize_groups();
        // Bring each group's front tab forward, the focused one last so it gets the focus.
        let active = map(session.active);
        for ix in [map(session.visible[0]), map(session.visible[1]), active].into_iter().flatten() {
            self.activate(ix, window, cx);
        }
        if active.is_none() && opened.iter().all(Option::is_none) {
            self.focus_group = 0;
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_round_trips_through_json() {
        let session = SavedSession {
            tabs: vec![SavedTab { path: "/a/b.cs".into(), group: 1, line: 41, col: 7 }],
            active: Some(0),
            visible: [None, Some(0)],
        };
        let text = serde_json::to_string(&session).unwrap();
        assert_eq!(serde_json::from_str::<SavedSession>(&text).unwrap(), session);
    }

    /// Open files, a split and carets survive closing everything and restoring the folder's session.
    #[gpui::test]
    fn reopens_the_files_with_their_carets(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = crate::workbench::settings_page::view_tests::workbench(cx, "session");
        std::fs::create_dir_all(guard.dir.join("proj")).unwrap();
        // The long form (Windows' temp folder can be an 8.3 short path), as the editor tabs hold it.
        let root = normalize_path(&guard.dir.join("proj"));
        let (a, b, c) = (root.join("a.txt"), root.join("b.txt"), root.join("c.txt"));
        std::fs::write(&a, "one\ntwo\nthree\nfour\n").unwrap();
        std::fs::write(&b, "x\ny\n").unwrap();
        std::fs::write(&c, "short\n").unwrap();
        wb.update_in(cx, |wb, window, cx| {
            wb.open_folder(root.clone(), window, cx);
            wb.open_file(a.clone(), Some(Point { line: 2, col: 3 }), window, cx);
            wb.open_file(c.clone(), Some(Point { line: 0, col: 5 }), window, cx);
            wb.focus_group = 1;
            wb.open_file(b.clone(), Some(Point { line: 1, col: 1 }), window, cx);
            wb.open_file(a.clone(), None, window, cx); // the focused tab: a.txt on the left
            wb.save_session(None, cx);
            for i in (0..wb.tabs.len()).rev() {
                wb.close_tab(i, window, cx);
            }
            assert!(wb.tabs.is_empty());
        });
        // The file shrank meanwhile: its caret is clamped.
        std::fs::write(&c, "").unwrap();
        cx.run_until_parked();
        wb.update_in(cx, |wb, window, cx| {
            wb.restore_session(window, cx);
            let open: Vec<_> = wb.tabs.iter().map(|t| (t.path.clone().unwrap(), t.group, t.editor.read(cx).state.caret())).collect();
            assert_eq!(
                open,
                vec![(a.clone(), 0, Point { line: 2, col: 3 }), (c.clone(), 0, Point { line: 0, col: 0 }), (b.clone(), 1, Point { line: 1, col: 1 })]
            );
            assert_eq!(wb.active, Some(0));
            assert_eq!(wb.visible, [Some(0), Some(2)]);
        });
        drop(guard);
    }

    #[test]
    fn window_bounds_round_trip() {
        let b = Bounds::new(gpui::point(px(10.), px(20.)), gpui::size(px(1200.), px(800.)));
        let saved = SavedWindow::from_bounds(WindowBounds::Maximized(b));
        assert_eq!(saved, SavedWindow { mode: "maximized".into(), x: 10., y: 20., width: 1200., height: 800. });
    }
}
