//! Editor tabs: opening, saving and closing files, and splitting the editor into groups.

use super::*;

impl Workbench {
    pub fn open_file(&mut self, path: PathBuf, at: Option<Point>, window: &mut Window, cx: &mut Context<Self>) {
        let path = normalize_path(&path);
        if let Some(i) = self.tabs.iter().position(|t| t.path.as_deref().is_some_and(|p| bf_core::paths::same_path(p, &path))) {
            self.activate(i, window, cx);
        } else {
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(e) => {
                    self.log(format!("Could not open {}: {e}", path.display()));
                    return;
                }
            };
            let bom = bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
            let body = if bom { &bytes[3..] } else { &bytes[..] };
            if body.iter().take(8000).any(|b| *b == 0) {
                self.log(format!("{} looks like a binary file and was not opened.", path.display()));
                self.show_panel(PanelView::Output, cx);
                return;
            }
            let text = String::from_utf8_lossy(body).into_owned();
            let editor = cx.new(|cx| EditorView::new(&text, Some(path.clone()), cx));
            editor.update(cx, |e, cx| {
                self.configure_editor(e);
                if e.language == Some(bf_syntax::Language::CSharp)
                    && let Some(lsp) = &self.lsp
                {
                    e.attach_lsp(lsp.clone(), cx);
                }
            });
            self.prepare_debuggable_editor(&editor, &path, cx);
            let title = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            self.add_tab(editor.clone(), Some(path.clone()), title, bom, window, cx);
            let marks = self.tests.read(cx).marks_for(&path);
            editor.update(cx, |e, cx| e.set_test_marks(marks, cx));
            self.load_git_base(editor, path.clone(), cx);
        }
        if let (Some(at), Some(editor)) = (at, self.active_editor()) {
            editor.update(cx, |e, cx| e.go_to(at, cx));
        }
    }

    pub(super) fn on_output_event(&mut self, _: &Entity<EditorView>, event: &EditorEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let EditorEvent::Navigate(path, at) = event {
            self.open_file(path.clone(), Some(*at), window, cx);
        }
    }

    pub(super) fn on_editor_event(&mut self, editor: &Entity<EditorView>, event: &EditorEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            EditorEvent::Navigate(path, point) => self.open_file(path.clone(), Some(*point), window, cx),
            EditorEvent::BreakpointsChanged => self.on_editor_breakpoints_changed(editor, cx),
            EditorEvent::ApplyCodeAction(action, then) => self.apply_code_action((**action).clone(), *then, editor.downgrade(), window, cx),
            EditorEvent::Locations { title, locations } => self.show_references(title.clone(), locations, cx),
            EditorEvent::RenameRequested { symbol, at } => {
                let kind = PromptKind::RenameSymbol { editor: editor.downgrade(), at: *at, old: symbol.clone() };
                self.open_prompt(kind, symbol, window, cx);
            }
            EditorEvent::ApplyWorkspaceEdit(edit) => self.apply_rename(edit, window, cx),
            EditorEvent::ShowCommit { commit, filename } => self.show_commit_of_file(commit.clone(), filename.clone(), window, cx),
            EditorEvent::Log(text) => {
                self.log(text);
                self.show_panel(PanelView::Output, cx);
            }
            EditorEvent::RunTestAt(line) => {
                if let Some(path) = self.tabs.iter().find(|t| t.editor == *editor).and_then(|t| t.path.clone()) {
                    self.run_test_at(&path, *line, false, window, cx);
                }
            }
            _ => cx.notify(),
        }
    }

    pub(super) fn add_tab(&mut self, editor: Entity<EditorView>, path: Option<PathBuf>, title: String, bom: bool, window: &mut Window, cx: &mut Context<Self>) {
        let sub = cx.subscribe_in(&editor, window, Self::on_editor_event);
        self.tabs.push(Tab { editor, group: self.focus_group, path, title, bom, page: None, _subs: vec![sub] });
        let ix = self.tabs.len() - 1;
        self.activate(ix, window, cx);
    }

    pub(super) fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix < self.tabs.len() {
            self.active = Some(ix);
            let group = self.tabs[ix].group;
            self.visible[group] = Some(ix);
            self.focus_group = group;
            let editor = self.tabs[ix].editor.clone();
            editor.update(cx, |e, _| e.focus(window));
            let path = self.tabs[ix].path.clone();
            self.explorer.update(cx, |e, cx| e.set_active(path.as_deref(), cx));
            self.solution.update(cx, |s, cx| s.set_active(path.as_deref(), cx));
            self.notify_editor_changed();
            cx.notify();
        }
    }

    pub(super) fn new_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.untitled += 1;
        let editor = cx.new(|cx| EditorView::new("", None, cx));
        editor.update(cx, |e, _| self.configure_editor(e));
        let title = format!("Untitled-{}", self.untitled);
        self.add_tab(editor, None, title, false, window, cx);
    }

    pub(super) fn save_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(ix) else { return };
        if tab.editor.read(cx).read_only {
            return;
        }
        let Some(path) = tab.path.clone() else {
            self.save_as(ix, window, cx);
            return;
        };
        let text = tab.editor.read(cx).save_path_text();
        let mut bytes = Vec::with_capacity(text.len() + 3);
        if tab.bom {
            bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
        }
        bytes.extend_from_slice(text.as_bytes());
        match std::fs::write(&path, bytes) {
            Ok(()) => {
                tab.editor.update(cx, |e, cx| {
                    e.state.buffer.mark_saved();
                    e.did_save();
                    cx.notify();
                });
                for p in &self.plugins {
                    p.event("fileSaved", Some(&path));
                }
                self.scm.update(cx, |s, cx| s.refresh(cx));
                if self.themes.current().file_path.as_deref() == Some(path.as_path()) || path.starts_with(self.paths.user_themes()) {
                    self.reload_theme(window, cx);
                }
            }
            Err(e) => {
                self.log(format!("Could not save {}: {e}", path.display()));
                self.show_panel(PanelView::Output, cx);
            }
        }
        cx.notify();
    }

    pub(super) fn save_as(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(ix) else { return };
        let dir = tab.path.as_ref().and_then(|p| p.parent().map(Path::to_path_buf)).or_else(|| self.workspace.root()).unwrap_or_else(|| PathBuf::from("."));
        let name = tab.title.clone();
        let rx = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await {
                let _ = this.update_in(cx, |wb, window, cx| {
                    let _ = std::fs::write(&path, "");
                    let path = normalize_path(&path);
                    if let Some(tab) = wb.tabs.get_mut(ix) {
                        tab.title = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        tab.path = Some(path.clone());
                        let text = tab.editor.read(cx).state.buffer.text();
                        // Recreate the editor so highlighting and language features match the new file type.
                        let editor = cx.new(|cx| EditorView::new(&text, Some(path.clone()), cx));
                        editor.update(cx, |e, cx| {
                            wb.configure_editor(e);
                            if e.language == Some(bf_syntax::Language::CSharp)
                                && let Some(lsp) = &wb.lsp
                            {
                                e.attach_lsp(lsp.clone(), cx);
                            }
                        });
                        wb.prepare_debuggable_editor(&editor, &path, cx);
                        let sub = cx.subscribe_in(&editor, window, Self::on_editor_event);
                        let tab = &mut wb.tabs[ix];
                        tab.editor = editor;
                        tab._subs = vec![sub];
                    }
                    wb.save_tab(ix, window, cx);
                    wb.activate(ix, window, cx);
                });
            }
        })
        .detach();
    }

    pub(super) fn save_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for ix in 0..self.tabs.len() {
            let tab = &self.tabs[ix];
            if tab.path.is_some() && tab.editor.read(cx).is_modified() {
                self.save_tab(ix, window, cx);
            }
        }
    }

    pub(super) fn close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(ix) else { return };
        if tab.editor.read(cx).is_modified() {
            let title = tab.title.clone();
            let rx = window.prompt(PromptLevel::Warning, &format!("Do you want to save the changes you made to {title}?"), None, &["Save", "Don't Save", "Cancel"], cx);
            cx.spawn_in(window, async move |this, cx| {
                if let Ok(choice) = rx.await {
                    let _ = this.update_in(cx, |wb, window, cx| match choice {
                        0 => {
                            wb.save_tab(ix, window, cx);
                            wb.remove_tab(ix, window, cx);
                        }
                        1 => wb.remove_tab(ix, window, cx),
                        _ => {}
                    });
                }
            })
            .detach();
            return;
        }
        self.remove_tab(ix, window, cx);
    }

    pub(super) fn remove_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() {
            return;
        }
        let tab = self.tabs.remove(ix);
        tab.editor.update(cx, |e, _| e.detach_lsp());
        for slot in &mut self.visible {
            *slot = match *slot {
                Some(j) if j == ix => None,
                Some(j) if j > ix => Some(j - 1),
                other => other,
            };
        }
        // The group the tab was in shows its neighbour (the tab before it, else the next one).
        if self.visible[tab.group].is_none() {
            let in_group: Vec<usize> = self.group_tabs(tab.group);
            self.visible[tab.group] = in_group.iter().rev().find(|&&j| j < ix).or(in_group.first()).copied();
        }
        self.normalize_groups();
        self.active = self.visible[self.focus_group];
        if let Some(i) = self.active {
            self.activate(i, window, cx);
        } else {
            window.focus(&self.focus);
            self.explorer.update(cx, |e, cx| e.set_active(None, cx));
            self.solution.update(cx, |s, cx| s.set_active(None, cx));
            self.notify_editor_changed();
        }
        cx.notify();
    }

    /// The file of the active tab (None for untitled documents and pages).
    pub(super) fn active_path(&self) -> Option<PathBuf> {
        self.active.and_then(|i| self.tabs.get(i)).and_then(|t| t.path.clone())
    }

    /// Tells modules which file is in front (`editorChanged`, path null when none is).
    pub(super) fn notify_editor_changed(&self) {
        let path = self.active_path();
        for p in &self.plugins {
            p.event("editorChanged", path.as_deref());
        }
    }
}

impl Workbench {
    pub(super) fn cycle_tab(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let in_group = self.group_tabs(self.focus_group);
        if in_group.is_empty() {
            return;
        }
        let pos = self.active.and_then(|a| in_group.iter().position(|&j| j == a)).unwrap_or(0);
        let i = in_group[(pos as isize + delta).rem_euclid(in_group.len() as isize) as usize];
        self.activate(i, window, cx);
    }

    // ───────────── editor groups (split editor) ─────────────

    /// Indexes of the tabs in `group`, in tab order.
    pub(super) fn group_tabs(&self, group: usize) -> Vec<usize> {
        self.tabs.iter().enumerate().filter(|(_, t)| t.group == group).map(|(i, _)| i).collect()
    }

    pub(super) fn is_split(&self) -> bool {
        self.tabs.iter().any(|t| t.group == 1)
    }

    /// Keeps the groups consistent: an empty left group takes over the right one, an empty focused
    /// group hands focus to the other.
    pub(super) fn normalize_groups(&mut self) {
        if self.group_tabs(0).is_empty() && self.is_split() {
            for t in &mut self.tabs {
                t.group = 0;
            }
            self.visible = [self.visible[1], None];
            self.focus_group = 0;
        }
        if self.group_tabs(self.focus_group).is_empty() {
            self.focus_group = 0;
        }
        for g in 0..2 {
            if self.visible[g].is_none_or(|i| self.tabs.get(i).is_none_or(|t| t.group != g)) {
                self.visible[g] = self.group_tabs(g).last().copied();
            }
        }
    }

    /// Ctrl+\\: moves the current editor into the other group (opening the right group when there is
    /// none). The only tab of a group can't leave it unless the other group exists.
    pub(super) fn split_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.active else { return };
        let from = self.tabs[ix].group;
        if !self.is_split() && self.group_tabs(0).len() < 2 {
            self.log("Split Editor: open a second file first; the current one moves into the right group.");
            return;
        }
        let to = 1 - from;
        self.tabs[ix].group = to;
        if self.visible[from] == Some(ix) {
            let left: Vec<usize> = self.group_tabs(from);
            self.visible[from] = left.iter().rev().find(|&&j| j < ix).or(left.first()).copied();
        }
        self.visible[to] = Some(ix);
        self.focus_group = to;
        self.normalize_groups();
        if let Some(i) = self.visible[self.focus_group] {
            self.activate(i, window, cx);
        }
        cx.notify();
    }

    /// Moves tab `from` into `group`: before tab `at` (after it, when moving right in the same group), or
    /// to the end of the group. A group's last tab can only leave it for the other, existing group.
    pub(super) fn move_tab(&mut self, from: usize, group: usize, at: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        if from >= self.tabs.len() || at == Some(from) {
            return;
        }
        let from_group = self.tabs[from].group;
        if group != from_group && self.group_tabs(from_group).len() == 1 && !self.is_split() {
            return; // the only tab can't split off on its own
        }
        let editors = |wb: &Self, ix: Option<usize>| ix.and_then(|i| wb.tabs.get(i)).map(|t| t.editor.clone());
        let visible = [editors(self, self.visible[0]), editors(self, self.visible[1])];
        let mut tab = self.tabs.remove(from);
        tab.group = group;
        let moved = tab.editor.clone();
        let index = match at {
            // Same group: the removal shifted the target, so this lands after it when moving right.
            Some(target) if group == from_group => target.min(self.tabs.len()),
            Some(target) => (if from < target { target - 1 } else { target }).min(self.tabs.len()),
            None => self.tabs.iter().rposition(|t| t.group == group).map_or(self.tabs.len(), |p| p + 1),
        };
        self.tabs.insert(index, tab);
        let find = |wb: &Self, e: &Option<Entity<EditorView>>| e.as_ref().and_then(|e| wb.tabs.iter().position(|t| t.editor == *e));
        self.visible = [find(self, &visible[0]), find(self, &visible[1])];
        let new_ix = self.tabs.iter().position(|t| t.editor == moved);
        if from_group != group && self.visible[from_group] == new_ix {
            self.visible[from_group] = None; // normalize_groups picks another tab there
        }
        self.visible[group] = new_ix;
        self.focus_group = group;
        self.normalize_groups();
        if let Some(ix) = self.tabs.iter().position(|t| t.editor == moved) {
            self.activate(ix, window, cx);
        }
        cx.notify();
    }

    pub(super) fn focus_group(&mut self, group: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self.visible[group].filter(|_| group == 0 || self.is_split()) {
            self.activate(i, window, cx);
        }
    }

    // ───────────── palette & prompts ─────────────
}
