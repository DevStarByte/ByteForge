//! Editor tabs: opening, saving and closing files, and splitting the editor into groups.

use super::*;

impl Workbench {
    pub fn open_file(&mut self, path: PathBuf, at: Option<Point>, window: &mut Window, cx: &mut Context<Self>) {
        let path = normalize_path(&path);
        let open = |t: &Tab| t.path.as_deref().is_some_and(|p| bf_core::paths::same_path(p, &path));
        // Open in both groups: the view in the focused group.
        let existing = self.tabs.iter().position(|t| t.group == self.focus_group && open(t)).or_else(|| self.tabs.iter().position(open));
        if let Some(i) = existing {
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

    /// Edits in a file open in both groups go to its other view (as minimal edits, so that view's caret and
    /// scroll stay; the echo back is a no-op).
    pub(super) fn sync_file_views(&mut self, editor: &Entity<EditorView>, cx: &mut Context<Self>) {
        let Some(path) = self.tabs.iter().find(|t| t.editor == *editor).and_then(|t| t.save_path(cx)) else { return };
        let others: Vec<Entity<EditorView>> = self.tabs.iter().filter(|t| t.editor != *editor && t.save_path(cx).is_some_and(|p| bf_core::paths::same_path(&p, &path))).map(|t| t.editor.clone()).collect();
        if others.is_empty() {
            return;
        }
        let (text, clean) = {
            let e = editor.read(cx);
            (e.state.buffer.text(), !e.is_modified())
        };
        // "Unmodified" alone isn't saved: a diff view opened from a tab with unsaved edits starts clean
        // with that tab's text. Only text that is what's on disk clears the other views' dirty marks.
        let clean = clean
            && std::fs::read_to_string(&path).is_ok_and(|disk| disk.trim_start_matches('\u{feff}').replace("\r\n", "\n") == text.replace("\r\n", "\n"));
        for other in others {
            other.update(cx, |e, cx| {
                e.replace_text_minimal(&text, cx);
                // The mirrored edit is a new undo step there: undoing back to the saved text, or a
                // format-on-save landing after the save, would otherwise leave that view dirty.
                if clean && e.is_modified() {
                    e.state.buffer.mark_saved();
                    cx.notify();
                }
            });
        }
    }

    /// "Open in Other Group": a second view of the current file in the other group (opening it on the
    /// right when there is one group); edits in either show in both.
    pub(super) fn open_in_other_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.active else { return };
        let Some(path) = self.tabs[ix].path.clone().filter(|_| self.tabs[ix].is_document()) else {
            self.log("Open in Other Group works on files.");
            return;
        };
        let to = 1 - self.tabs[ix].group;
        if let Some(existing) = self.tabs.iter().position(|t| t.group == to && t.path.as_deref().is_some_and(|p| bf_core::paths::same_path(p, &path))) {
            return self.activate(existing, window, cx);
        }
        let source = self.tabs[ix].editor.clone();
        let (text, bom, caret) = {
            let e = source.read(cx);
            (e.state.buffer.text(), self.tabs[ix].bom, e.state.caret())
        };
        // Start from the file on disk, then take the unsaved edits, so the new view is dirty when the first is.
        let saved = std::fs::read(&path).map(|b| String::from_utf8_lossy(b.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(&b)).into_owned()).unwrap_or_else(|_| text.clone());
        let editor = cx.new(|cx| EditorView::new(&saved, Some(path.clone()), cx));
        editor.update(cx, |e, cx| {
            self.configure_editor(e);
            e.replace_text_minimal(&text, cx);
            if !e.is_modified() || saved == text {
                e.state.buffer.mark_saved();
            }
            e.lsp_mirror = true;
            if e.language == Some(bf_syntax::Language::CSharp)
                && let Some(lsp) = &self.lsp
            {
                e.attach_lsp(lsp.clone(), cx);
            }
        });
        self.prepare_debuggable_editor(&editor, &path, cx);
        let title = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.focus_group = to;
        self.add_tab(editor.clone(), Some(path.clone()), title, bom, window, cx);
        let marks = self.tests.read(cx).marks_for(&path);
        editor.update(cx, |e, cx| {
            e.set_test_marks(marks, cx);
            e.go_to(caret, cx);
        });
        self.load_git_base(editor, path, cx);
        self.normalize_groups();
        cx.notify();
    }

    pub(super) fn on_output_event(&mut self, _: &Entity<EditorView>, event: &EditorEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let EditorEvent::Navigate(path, at) = event {
            self.open_file(path.clone(), Some(*at), window, cx);
        }
    }

    pub(super) fn on_editor_event(&mut self, editor: &Entity<EditorView>, event: &EditorEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            EditorEvent::Navigate(path, point) => self.open_file(path.clone(), Some(*point), window, cx),
            EditorEvent::Edited => {
                self.sync_file_views(editor, cx);
                self.schedule_autosave(editor, window, cx);
                cx.notify();
            }
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
        // files.autoSave "onFocusChange" (and "onWindowChange"): switching away saves the file left.
        if let Some(previous) = self.active.filter(|&p| p != ix && p < self.tabs.len())
            && matches!(self.autosave_mode(), AutoSave::OnFocusChange | AutoSave::OnWindowChange)
            && self.tabs[previous].save_path(cx).is_some()
            && self.tabs[previous].editor.read(cx).is_modified()
        {
            self.save_tab(previous, window, cx);
        }
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
        let Some(path) = tab.save_path(cx) else {
            self.save_as(ix, window, cx);
            return;
        };
        let text = tab.editor.read(cx).save_path_text();
        let mut bytes = Vec::with_capacity(text.len() + 3);
        // A diff tab doesn't know the file's BOM: keep what the file has.
        let bom = if tab.path.is_some() { tab.bom } else { std::fs::read(&path).is_ok_and(|b| b.starts_with(&[0xEF, 0xBB, 0xBF])) };
        if bom {
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
                // The same file open in the other group is saved too.
                let others: Vec<Entity<EditorView>> = self.tabs.iter().filter(|t| t.editor != tab.editor && t.save_path(cx).is_some_and(|p| bf_core::paths::same_path(&p, &path))).map(|t| t.editor.clone()).collect();
                for other in others {
                    other.update(cx, |e, cx| {
                        if e.save_path_text() == text {
                            e.state.buffer.mark_saved();
                            cx.notify();
                        }
                    });
                }
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

    /// The "files.autoSave" setting.
    pub(super) fn autosave_mode(&self) -> AutoSave {
        match self.settings.get_string("files.autoSave").as_deref() {
            Some("afterDelay") => AutoSave::AfterDelay,
            Some("onFocusChange") => AutoSave::OnFocusChange,
            Some("onWindowChange") => AutoSave::OnWindowChange,
            _ => AutoSave::Off,
        }
    }

    /// files.autoSave "afterDelay": saves the file once typing pauses for "files.autoSaveDelay" ms
    /// (without formatting, so the text doesn't change under the caret).
    fn schedule_autosave(&mut self, editor: &Entity<EditorView>, window: &mut Window, cx: &mut Context<Self>) {
        if self.autosave_mode() != AutoSave::AfterDelay {
            return;
        }
        let delay: u64 = self.settings.get("files.autoSaveDelay", 1000u64).max(100);
        let target = editor.downgrade();
        let task = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(delay)).await;
            let _ = this.update_in(cx, |wb, window, cx| {
                let Some(editor) = target.upgrade() else { return };
                wb.autosave.remove(&editor.entity_id());
                if let Some(ix) = wb.tabs.iter().position(|t| t.editor == editor && t.save_path(cx).is_some())
                    && !editor.read(cx).read_only
                    && editor.read(cx).is_modified()
                {
                    wb.save_tab(ix, window, cx);
                }
            });
        });
        // A newer edit replaces (and so cancels) the pending save.
        self.autosave.insert(editor.entity_id(), task);
    }

    pub(super) fn save_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for ix in 0..self.tabs.len() {
            let tab = &self.tabs[ix];
            if tab.save_path(cx).is_some() && tab.editor.read(cx).is_modified() {
                self.save_tab(ix, window, cx);
            }
        }
    }

    pub(super) fn close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(ix) else { return };
        // Another view of the file stays open with the same unsaved edits: nothing is lost, so don't ask.
        let other_view = tab.save_path(cx).is_some_and(|p| self.tabs.iter().any(|t| t.editor != tab.editor && t.save_path(cx).is_some_and(|q| bf_core::paths::same_path(&p, &q))));
        if tab.editor.read(cx).is_modified() && !other_view {
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
        // The file stays open in the other group: that view keeps the server's document now.
        if let Some(path) = &tab.path
            && let Some(other) = self.tabs.iter().find(|t| t.path.as_deref().is_some_and(|p| bf_core::paths::same_path(p, path)))
        {
            other.editor.update(cx, |e, cx| e.take_over_lsp(cx));
        }
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
    /// none). The only tab can't leave its group, so then (as in VS Code) the file shows in both groups.
    pub(super) fn split_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.active else { return };
        let from = self.tabs[ix].group;
        if !self.is_split() && self.group_tabs(0).len() < 2 {
            return self.open_in_other_group(window, cx);
        }
        let to = 1 - from;
        if self.join_view_in_group(ix, to, window, cx) {
            return;
        }
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
        if group != from_group && self.join_view_in_group(from, group, window, cx) {
            return;
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

    /// Moving tab `from` into `group`, which already has a view of its file: one view there is enough
    /// (they share edits), so this one closes and that one comes to the front. False when there is none.
    fn join_view_in_group(&mut self, from: usize, group: usize, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(path) = self.tabs[from].path.clone().filter(|_| self.tabs[from].is_document()) else { return false };
        let Some(existing) = self.tabs.iter().find(|t| t.group == group && t.is_document() && t.path.as_deref().is_some_and(|p| bf_core::paths::same_path(p, &path))).map(|t| t.editor.clone()) else {
            return false;
        };
        self.remove_tab(from, window, cx);
        if let Some(ix) = self.tabs.iter().position(|t| t.editor == existing) {
            self.activate(ix, window, cx);
        }
        true
    }

    pub(super) fn focus_group(&mut self, group: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self.visible[group].filter(|_| group == 0 || self.is_split()) {
            self.activate(i, window, cx);
        }
    }

    // ───────────── palette & prompts ─────────────
}

/// When files save by themselves ("files.autoSave").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AutoSave {
    Off,
    AfterDelay,
    OnFocusChange,
    OnWindowChange,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workbench::settings_page::view_tests::workbench;
    use gpui::{Modifiers, VisualTestContext};

    /// A folder with `names` (each file holds its own name) open in the workbench, every file opened in turn.
    fn open_files(wb: &Entity<Workbench>, cx: &mut VisualTestContext, dir: &Path, names: &[&str]) {
        std::fs::create_dir_all(dir).unwrap();
        for n in names {
            std::fs::write(dir.join(n), format!("{n}\n")).unwrap();
        }
        wb.update_in(cx, |wb, window, cx| wb.open_folder(dir.to_path_buf(), window, cx));
        cx.run_until_parked();
        for n in names {
            wb.update_in(cx, |wb, window, cx| wb.open_file(dir.join(n), None, window, cx));
            cx.run_until_parked();
        }
    }

    /// The tabs as (group, title), in tab order.
    fn tabs(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Vec<(usize, String)> {
        wb.read_with(cx, |wb, _| wb.tabs.iter().map(|t| (t.group, t.title.clone())).collect())
    }

    /// Draws the window as it is now (mouse events hit-test the last drawn frame, and the test window
    /// only draws when asked).
    fn redraw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear());
    }

    fn bounds(cx: &mut VisualTestContext, selector: String) -> gpui::Bounds<gpui::Pixels> {
        let selector: &'static str = Box::leak(selector.into_boxed_str());
        redraw(cx);
        cx.debug_bounds(selector).unwrap_or_else(|| panic!("{selector} is drawn"))
    }

    /// Presses the mouse on `from`, moves it (past the drag threshold) to `to` and lets go there.
    fn drag(cx: &mut VisualTestContext, from: gpui::Point<gpui::Pixels>, to: gpui::Point<gpui::Pixels>) {
        cx.simulate_mouse_move(from, None, Modifiers::none());
        cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(from + gpui::point(px(10.), px(0.)), MouseButton::Left, Modifiers::none());
        redraw(cx);
        cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::none());
        redraw(cx);
        cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
        redraw(cx);
    }

    #[gpui::test]
    fn dragging_a_tab_onto_another_reorders_them(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "tab-reorder");
        open_files(&wb, cx, &guard.dir.join("ws"), &["a.txt", "b.txt", "c.txt"]);
        let (c, a) = (bounds(cx, "tab-0-c.txt".into()), bounds(cx, "tab-0-a.txt".into()));
        drag(cx, c.center(), a.center());
        assert_eq!(tabs(&wb, cx), vec![(0, "c.txt".into()), (0, "a.txt".into()), (0, "b.txt".into())]);
        // And to the right: a onto b puts it after b.
        let (a, b) = (bounds(cx, "tab-0-a.txt".into()), bounds(cx, "tab-0-b.txt".into()));
        drag(cx, a.center(), b.center());
        assert_eq!(tabs(&wb, cx), vec![(0, "c.txt".into()), (0, "b.txt".into()), (0, "a.txt".into())]);
        drop(guard);
    }

    #[gpui::test]
    fn dragging_a_tab_onto_the_editor_opens_the_right_group(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "tab-to-right");
        open_files(&wb, cx, &guard.dir.join("ws"), &["a.txt", "b.txt"]);
        let (a, content) = (bounds(cx, "tab-0-a.txt".into()), bounds(cx, "editor-content".into()));
        drag(cx, a.center(), content.center());
        assert_eq!(tabs(&wb, cx), vec![(0, "b.txt".into()), (1, "a.txt".into())]);
        // Back into the left group, onto its tab.
        let (a, b) = (bounds(cx, "tab-1-a.txt".into()), bounds(cx, "tab-0-b.txt".into()));
        drag(cx, a.center(), b.center());
        assert_eq!(tabs(&wb, cx), vec![(0, "a.txt".into()), (0, "b.txt".into())]);
        assert!(!wb.read_with(cx, |wb, _| wb.is_split()), "the empty right group closes");
        drop(guard);
    }

    #[gpui::test]
    fn dragging_a_tab_into_the_other_group(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "tab-other-group");
        open_files(&wb, cx, &guard.dir.join("ws"), &["a.txt", "b.txt", "c.txt"]);
        cx.simulate_keystrokes("ctrl-\\");
        cx.run_until_parked();
        assert_eq!(tabs(&wb, cx), vec![(0, "a.txt".into()), (0, "b.txt".into()), (1, "c.txt".into())]);
        // Onto the right group's empty strip: to its end.
        let (a, strip) = (bounds(cx, "tab-0-a.txt".into()), bounds(cx, "tab-strip-1".into()));
        drag(cx, a.center(), gpui::point(strip.right() - px(20.), strip.center().y));
        assert_eq!(tabs(&wb, cx), vec![(0, "b.txt".into()), (1, "c.txt".into()), (1, "a.txt".into())]);
        // Onto the left group's editor.
        let (c, body) = (bounds(cx, "tab-1-c.txt".into()), bounds(cx, "editor-group-body-0".into()));
        drag(cx, c.center(), body.center());
        assert_eq!(tabs(&wb, cx), vec![(0, "b.txt".into()), (0, "c.txt".into()), (1, "a.txt".into())]);
        wb.read_with(cx, |wb, _| {
            assert_eq!(wb.active.map(|i| wb.tabs[i].title.as_str()), Some("c.txt"), "the dropped tab is in front");
            assert_eq!(wb.focus_group, 0);
        });
        drop(guard);
    }

    /// The text and dirty state of every view of `title`, in tab order.
    fn views(wb: &Entity<Workbench>, cx: &mut VisualTestContext, title: &str) -> Vec<(usize, String, bool)> {
        wb.read_with(cx, |wb, cx| {
            wb.tabs.iter().filter(|t| t.title == title).map(|t| (t.group, t.editor.read(cx).state.buffer.text(), t.editor.read(cx).is_modified())).collect()
        })
    }

    fn click(cx: &mut VisualTestContext, selector: String) {
        let b = bounds(cx, selector);
        cx.simulate_click(b.center(), Modifiers::none());
        redraw(cx);
    }

    /// Ctrl+\ with a single file shows it in both groups (it used to only log that a second file was needed).
    #[gpui::test]
    fn split_editor_with_one_file_shows_it_in_both_groups(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "split-one");
        open_files(&wb, cx, &guard.dir.join("ws"), &["a.txt"]);
        cx.simulate_keystrokes("ctrl-\\");
        cx.run_until_parked();
        assert_eq!(tabs(&wb, cx), vec![(0, "a.txt".into()), (1, "a.txt".into())]);
        drop(guard);
    }

    /// One file in both groups: typing in either shows in both, saving either saves the file and clears
    /// both dirty marks, undo back to the saved text clears both, and closing one view keeps the other
    /// (without asking to save, since the other view still has the edits).
    #[gpui::test]
    fn one_file_in_both_groups_stays_in_sync(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "both-groups");
        let dir = guard.dir.join("ws");
        open_files(&wb, cx, &dir, &["a.txt", "b.txt"]);
        click(cx, "tab-0-a.txt".into());
        cx.dispatch_action(crate::actions::OpenInOtherGroup);
        cx.run_until_parked();
        assert_eq!(tabs(&wb, cx), vec![(0, "a.txt".into()), (0, "b.txt".into()), (1, "a.txt".into())]);
        // Opening the file again (Explorer, Quick Open) stays in the focused group's view.
        wb.update_in(cx, |wb, window, cx| wb.open_file(dir.join("a.txt"), None, window, cx));
        cx.run_until_parked();
        wb.read_with(cx, |wb, _| assert_eq!(wb.active, Some(2)));

        cx.simulate_input("X");
        cx.run_until_parked();
        assert_eq!(views(&wb, cx, "a.txt"), vec![(0, "Xa.txt\n".into(), true), (1, "Xa.txt\n".into(), true)]);
        cx.simulate_keystrokes("ctrl-s");
        cx.run_until_parked();
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "Xa.txt\n");
        assert_eq!(views(&wb, cx, "a.txt"), vec![(0, "Xa.txt\n".into(), false), (1, "Xa.txt\n".into(), false)]);

        // The left view: type, then undo back to the saved text.
        click(cx, "tab-0-a.txt".into());
        cx.simulate_input("Y");
        cx.run_until_parked();
        assert_eq!(views(&wb, cx, "a.txt"), vec![(0, "XYa.txt\n".into(), true), (1, "XYa.txt\n".into(), true)]);
        cx.simulate_keystrokes("ctrl-z");
        cx.run_until_parked();
        assert_eq!(views(&wb, cx, "a.txt"), vec![(0, "Xa.txt\n".into(), false), (1, "Xa.txt\n".into(), false)]);

        // Unsaved edit, then close the right view: no question, the left keeps the edit.
        cx.simulate_input("Z");
        cx.run_until_parked();
        click(cx, "tab-1-a.txt".into());
        cx.simulate_keystrokes("ctrl-w");
        cx.run_until_parked();
        assert!(!cx.has_pending_prompt(), "closing one of two views doesn't ask to save");
        assert_eq!(tabs(&wb, cx), vec![(0, "a.txt".into()), (0, "b.txt".into())]);
        assert_eq!(views(&wb, cx, "a.txt"), vec![(0, "XZa.txt\n".into(), true)]);
        // The last view does ask.
        click(cx, "tab-0-a.txt".into());
        cx.simulate_keystrokes("ctrl-w");
        cx.run_until_parked();
        assert!(cx.has_pending_prompt(), "closing the last view of an unsaved file asks");
        cx.simulate_prompt_answer("Save");
        cx.run_until_parked();
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "XZa.txt\n");
        assert_eq!(tabs(&wb, cx), vec![(0, "b.txt".into())]);
        drop(guard);
    }

    /// Dragging a view of a file into the group that already shows that file doesn't leave the file
    /// twice in one group: the dragged view closes and the one there comes to the front.
    #[gpui::test]
    fn dragging_a_view_onto_its_twin_group_joins_them(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "join-views");
        open_files(&wb, cx, &guard.dir.join("ws"), &["a.txt", "b.txt"]);
        click(cx, "tab-0-a.txt".into());
        cx.dispatch_action(crate::actions::OpenInOtherGroup);
        cx.run_until_parked();
        click(cx, "tab-0-b.txt".into());
        let (a, strip) = (bounds(cx, "tab-1-a.txt".into()), bounds(cx, "tab-strip-0".into()));
        drag(cx, a.center(), gpui::point(strip.right() - px(20.), strip.center().y));
        assert_eq!(tabs(&wb, cx), vec![(0, "a.txt".into()), (0, "b.txt".into())]);
        wb.read_with(cx, |wb, _| assert_eq!(wb.active.map(|i| wb.tabs[i].title.as_str()), Some("a.txt")));
        // Ctrl+\ (move to the other group) does the same.
        click(cx, "tab-0-a.txt".into());
        cx.dispatch_action(crate::actions::OpenInOtherGroup);
        cx.run_until_parked();
        assert_eq!(tabs(&wb, cx), vec![(0, "a.txt".into()), (0, "b.txt".into()), (1, "a.txt".into())]);
        cx.simulate_keystrokes("ctrl-\\");
        cx.run_until_parked();
        assert_eq!(tabs(&wb, cx), vec![(0, "a.txt".into()), (0, "b.txt".into())]);
        drop(guard);
    }

    /// The labels in the open breadcrumb dropdown, with the current one starred (None when closed).
    fn crumb_menu(wb: &Entity<Workbench>, cx: &mut VisualTestContext) -> Option<Vec<String>> {
        wb.read_with(cx, |wb, _| wb.crumb_menu_labels())
    }

    /// The breadcrumbs show the symbols around the caret; clicking one lists the symbols next to it, and
    /// picking one goes to it. With two groups each group has its own breadcrumbs, of the file it shows.
    #[gpui::test]
    fn clicking_a_breadcrumb_goes_to_the_symbol(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "breadcrumbs");
        let dir = guard.dir.join("ws");
        open_files(&wb, cx, &dir, &["b.txt"]);
        std::fs::write(dir.join("a.txt"), "class Foo\n{\n    void Bar()\n    {\n    }\n    void Baz()\n    {\n    }\n}\n").unwrap();
        wb.update_in(cx, |wb, window, cx| wb.open_file(dir.join("a.txt"), None, window, cx));
        cx.run_until_parked();
        let pos = |line, character| bf_lsp::Position { line, character };
        let symbol = |name: &str, kind, start: (u32, u32), full: ((u32, u32), (u32, u32)), depth| bf_lsp::Symbol {
            name: name.into(),
            kind,
            detail: None,
            container: None,
            path: dir.join("a.txt"),
            range: bf_lsp::Range { start: pos(start.0, start.1), end: pos(start.0, start.1 + name.len() as u32) },
            full_range: bf_lsp::Range { start: pos(full.0.0, full.0.1), end: pos(full.1.0, full.1.1) },
            depth,
        };
        let outline = vec![
            symbol("Foo", "class", (0, 6), ((0, 0), (8, 1)), 0),
            symbol("Bar", "method", (2, 9), ((2, 4), (4, 5)), 1),
            symbol("Baz", "method", (5, 9), ((5, 4), (7, 5)), 1),
        ];
        let editor = wb.read_with(cx, |wb, _| wb.active_editor().unwrap());
        editor.update(cx, |e, cx| {
            e.set_outline_for_test(outline.clone());
            e.go_to(Point::new(3, 4), cx);
        });
        cx.run_until_parked();
        redraw(cx);
        assert!(cx.debug_bounds("crumb-0-Bar").is_some(), "the method is in the breadcrumbs");
        click(cx, "crumb-0-Foo".into());
        assert_eq!(crumb_menu(&wb, cx), Some(vec!["*Foo".to_string()]));
        click(cx, "crumb-item-Foo".into());
        assert_eq!(crumb_menu(&wb, cx), None);
        assert_eq!(editor.read_with(cx, |e, _| e.state.caret()), Point::new(0, 6));
        // (debug_bounds keeps elements of earlier frames, so this asks the editor.)
        let around: Vec<String> = editor.read_with(cx, |e, _| e.symbol_path().into_iter().map(|s| s.0).collect());
        assert_eq!(around, vec!["Foo".to_string()], "only the class is around the caret now");

        // Two groups: b.txt left, a.txt right; the right group's crumbs work there.
        cx.simulate_keystrokes("ctrl-\\");
        cx.run_until_parked();
        assert_eq!(tabs(&wb, cx), vec![(0, "b.txt".into()), (1, "a.txt".into())]);
        editor.update(cx, |e, cx| e.go_to(Point::new(3, 4), cx));
        click(cx, "tab-0-b.txt".into());
        click(cx, "crumb-1-Bar".into());
        assert_eq!(crumb_menu(&wb, cx), Some(vec!["*Bar".to_string(), "Baz".into()]), "the methods of the class");
        wb.read_with(cx, |wb, _| assert_eq!(wb.active.map(|i| wb.tabs[i].title.as_str()), Some("a.txt"), "the clicked group is in front"));
        // Down and Enter pick the next one.
        cx.simulate_keystrokes("down enter");
        redraw(cx);
        assert_eq!(crumb_menu(&wb, cx), None);
        assert_eq!(editor.read_with(cx, |e, _| e.state.caret()), Point::new(5, 9));
        drop(guard);
    }

    /// A folder crumb lists the folder (as the Explorer does); a folder in it drills down, a file opens in
    /// the crumb's group. The file crumb lists its siblings. Escape and a second click close the list.
    #[gpui::test]
    fn breadcrumb_folders_list_their_entries(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "breadcrumb-folders");
        let ws = guard.dir.join("ws");
        for d in ["src/sub", "src/obj", "src/Zeta"] {
            std::fs::create_dir_all(ws.join(d)).unwrap();
        }
        for f in ["src/a.txt", "src/b.txt", "src/sub/c.txt", "README.md"] {
            std::fs::write(ws.join(f), "x\n").unwrap();
        }
        wb.update_in(cx, |wb, window, cx| wb.open_folder(ws.clone(), window, cx));
        cx.run_until_parked();
        wb.update_in(cx, |wb, window, cx| wb.open_file(ws.join("src/a.txt"), None, window, cx));
        redraw(cx);

        // Folders first, case-insensitive, `obj` left out; the way to the file is marked.
        click(cx, "crumb-0-src".into());
        assert_eq!(crumb_menu(&wb, cx), Some(vec!["sub".to_string(), "Zeta".into(), "*a.txt".into(), "b.txt".into()]));
        // Clicking the crumb again closes it.
        click(cx, "crumb-0-src".into());
        assert_eq!(crumb_menu(&wb, cx), None);
        click(cx, "crumb-0-src".into());
        click(cx, "crumb-item-sub".into());
        assert_eq!(crumb_menu(&wb, cx), Some(vec!["c.txt".to_string()]), "drilled into sub");
        click(cx, "crumb-item-c.txt".into());
        assert_eq!(crumb_menu(&wb, cx), None);
        assert_eq!(tabs(&wb, cx), vec![(0, "a.txt".into()), (0, "c.txt".into())]);

        // The file crumb: its folder, itself marked; Escape closes and gives the editor the keys back.
        click(cx, "crumb-0-c.txt".into());
        assert_eq!(crumb_menu(&wb, cx), Some(vec!["*c.txt".to_string()]));
        cx.simulate_keystrokes("escape");
        redraw(cx);
        assert_eq!(crumb_menu(&wb, cx), None);
        let focused = wb.update_in(cx, |wb, window, cx| wb.active_editor().unwrap().read(cx).focus_handle(cx).is_focused(window));
        assert!(focused, "the editor has the focus again");

        // Keyboard: src lists sub (marked), Zeta, a.txt, b.txt; Down twice and Enter opens a.txt.
        click(cx, "crumb-0-src".into());
        cx.simulate_keystrokes("down down enter");
        redraw(cx);
        wb.read_with(cx, |wb, _| assert_eq!(wb.active.map(|i| wb.tabs[i].title.as_str()), Some("a.txt")));

        // Two groups: a file picked in the right group's crumbs opens there, though the left has it too.
        wb.update_in(cx, |wb, window, cx| {
            let c = wb.tabs.iter().position(|t| t.title == "c.txt").unwrap();
            wb.move_tab(c, 1, None, window, cx)
        });
        redraw(cx);
        click(cx, "crumb-1-src".into());
        click(cx, "crumb-item-b.txt".into());
        assert_eq!(tabs(&wb, cx), vec![(0, "a.txt".into()), (1, "c.txt".into()), (1, "b.txt".into())]);
        click(cx, "crumb-1-src".into());
        click(cx, "crumb-item-a.txt".into());
        assert_eq!(tabs(&wb, cx), vec![(0, "a.txt".into()), (1, "c.txt".into()), (1, "b.txt".into()), (1, "a.txt".into())]);
        wb.read_with(cx, |wb, _| assert_eq!(wb.active.map(|i| (wb.tabs[i].group, wb.tabs[i].title.as_str())), Some((1, "a.txt"))));
        drop(guard);
    }
}
