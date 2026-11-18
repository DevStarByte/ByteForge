//! Source control events, git gutter bases and diff tabs.

use super::*;
use crate::diff_view::{ChangeAction, DiffKind};

/// The diff tab's title: the file name and which comparison it is.
fn diff_title(path: &Path, staged: bool, untracked: bool) -> String {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    format!("{name} ({})", if staged { "Staged" } else if untracked { "Untracked" } else { "Working Tree" })
}

/// What the two sides of a working-tree or staged diff are called.
fn diff_labels(staged: bool, untracked: bool) -> (String, String) {
    if staged {
        ("HEAD".to_owned(), "Index (staged)".to_owned())
    } else if untracked {
        ("(new file)".to_owned(), "Working Tree".to_owned())
    } else {
        ("Index".to_owned(), "Working Tree".to_owned())
    }
}

impl Workbench {
    pub(super) fn on_scm_event(&mut self, scm: &Entity<ScmView>, event: &ScmEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            ScmEvent::OpenFile(path) => self.open_file(path.clone(), None, window, cx),
            ScmEvent::OpenDiff(change, staged) => self.open_diff(change.clone(), *staged, window, cx),
            ScmEvent::OpenCommitDiff(commit, file) => self.open_commit_diff(commit.clone(), file.clone(), window, cx),
            ScmEvent::Log(text) => self.log(text),
            ScmEvent::CheckoutFailed(branch, message) => self.checkout_failed(branch.clone(), message.clone(), window, cx),
            ScmEvent::StatusChanged => {
                let s = scm.read(cx);
                let decorations = match s.git() {
                    Some(git) => crate::scm::decorations(s.status(), git.root()),
                    None => HashMap::new(),
                };
                let head = Some(s.status().head.clone());
                self.explorer.update(cx, |e, cx| e.set_decorations(decorations, cx));
                // Pull, checkout and discard rewrite files: show the new content in unmodified editors.
                self.reload_changed_tabs(cx);
                // Bases change with HEAD, and when files become tracked or untracked.
                self.git_head = head;
                self.update_git_bases(cx);
                cx.notify();
            }
        }
    }

    /// Loads the HEAD version of an open file for the gutter change markers (off the UI thread).
    pub(super) fn load_git_base(&self, editor: Entity<EditorView>, path: PathBuf, cx: &mut Context<Self>) {
        let scm = self.scm.read(cx);
        let Some(git) = scm.git().cloned() else {
            editor.update(cx, |e, cx| e.set_git_base(None, cx));
            return;
        };
        if !path.starts_with(git.root()) {
            return;
        }
        let new_file = scm.status().change_for(&path).is_some_and(|c| c.untracked || c.index == 'A');
        let blame = self.settings.get("git.blame.currentLine", true);
        editor.update(cx, |e, cx| e.set_git_repo(Some(git.clone()), blame && !new_file, cx));
        let editor = editor.downgrade();
        cx.spawn(async move |_, cx| {
            let base = cx.background_executor().spawn(async move { if new_file { Some(String::new()) } else { git.head_content(&path) } }).await;
            let _ = editor.update(cx, |e, cx| e.set_git_base(base, cx));
        })
        .detach();
    }

    pub(super) fn update_git_bases(&self, cx: &mut Context<Self>) {
        for tab in &self.tabs {
            if let Some(path) = &tab.path
                && !tab.editor.read(cx).read_only
            {
                self.load_git_base(tab.editor.clone(), path.clone(), cx);
            }
        }
    }

    /// Reloads unmodified editors whose file changed on disk (and the working-tree side of diffs).
    pub(super) fn reload_changed_tabs(&self, cx: &mut Context<Self>) {
        for tab in &self.tabs {
            let Some(path) = &tab.save_path(cx) else { continue };
            let e = tab.editor.read(cx);
            if e.is_modified() || e.read_only {
                continue;
            }
            let Ok(bytes) = std::fs::read(path) else { continue };
            let body = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(&bytes);
            let text = String::from_utf8_lossy(body);
            if text != e.save_path_text() {
                tab.editor.update(cx, |e, cx| e.reload_text(&text, cx));
            }
        }
    }

    /// Opens a change in the diff editor (reusing its tab when it is already open).
    pub(super) fn open_diff(&mut self, change: bf_core::git::FileChange, staged: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(git) = self.scm.read(cx).git().cloned() else { return };
        let title = diff_title(&change.path, staged, change.untracked);
        let labels = diff_labels(staged, change.untracked);
        let kind = if staged { DiffKind::Staged } else { DiffKind::WorkingTree };
        let path = change.path.clone();
        let task = cx.background_spawn(async move { git.diff_texts(&change, staged) });
        self.show_diff_tab(title, path, labels, kind, task, window, cx);
    }

    /// Opens what a commit changed in a file (first parent ↔ commit) in the diff editor.
    pub(super) fn open_commit_diff(&mut self, commit: bf_core::git::Commit, file: bf_core::git::CommitFile, window: &mut Window, cx: &mut Context<Self>) {
        let Some(git) = self.scm.read(cx).git().cloned() else { return };
        let name = file.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let title = format!("{name} ({})", commit.short());
        let old = match commit.parents.first() {
            Some(p) if file.status != 'A' => p[..p.len().min(7)].to_owned(),
            _ => "(new file)".to_owned(),
        };
        let new = if file.status == 'D' { "(deleted)".to_owned() } else { commit.short().to_owned() };
        let path = file.path.clone();
        let task = cx.background_spawn(async move { git.commit_diff_texts(&commit, &file) });
        self.show_diff_tab(title, path, (old, new), DiffKind::ReadOnly, task, window, cx);
    }

    /// Shows `(old, new)` from `task` in a diff editor tab titled `title`, reusing an open one of the same
    /// title. `path` is the file compared: its language highlights both sides, and "Open File" opens it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn show_diff_tab(&mut self, title: String, path: PathBuf, labels: (String, String), kind: DiffKind, task: Task<(String, String)>, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let (old, new) = task.await;
            let _ = this.update_in(cx, |wb, window, cx| {
                let file = path.exists().then(|| path.clone());
                let language = bf_syntax::Language::from_path(&path);
                // The working tree as it is in an open editor, unsaved edits included.
                let new = match kind {
                    DiffKind::WorkingTree => wb.tabs.iter().find(|t| t.is_document() && t.path.as_deref().is_some_and(|p| bf_core::paths::same_path(p, &path))).map(|t| t.editor.read(cx).state.buffer.text()).unwrap_or(new),
                    _ => new,
                };
                // Same title and same file: two `Program.cs` in different folders get a tab each.
                let existing = wb.tabs.iter().position(|t| t.title == title && matches!(&t.page, Some(Page::Diff(d)) if d.read(cx).path() == path));
                if let Some(ix) = existing
                    && let Some(Page::Diff(diff)) = wb.tabs[ix].page.clone()
                {
                    diff.update(cx, |d, cx| {
                        d.set_language(language, cx);
                        d.set_texts(&old, &new, labels, file, kind, cx);
                    });
                    wb.activate(ix, window, cx);
                    return;
                }
                let editor = cx.new(|cx| EditorView::new("", None, cx));
                let diff = cx.new(|cx| crate::diff_view::DiffView::new(editor.clone(), path.clone(), language, cx));
                let sub = cx.subscribe_in(&diff, window, |wb: &mut Self, diff, event: &crate::diff_view::DiffEvent, window, cx| match event {
                    crate::diff_view::DiffEvent::OpenFile(path, at) => wb.open_file(path.clone(), Some(*at), window, cx),
                    crate::diff_view::DiffEvent::Change(action, path, change) => wb.diff_change(diff.clone(), *action, path.clone(), *change, window, cx),
                    crate::diff_view::DiffEvent::SaveThenInline => {
                        if let Some(ix) = wb.tabs.iter().position(|t| matches!(&t.page, Some(Page::Diff(d)) if d == diff)) {
                            wb.save_tab(ix, window, cx);
                        }
                        diff.update(cx, |d, cx| d.toggle_inline(window, cx));
                    }
                });
                wb.add_tab(editor, None, title, false, window, cx);
                if let Some(tab) = wb.tabs.last_mut() {
                    tab.page = Some(Page::Diff(diff.clone()));
                    tab._subs.push(sub);
                }
                // After the tab is showing, so the editors know their size when they scroll to the first change.
                diff.update(cx, |d, cx| d.set_texts(&old, &new, labels, file, kind, cx));
                cx.notify();
            });
        })
        .detach();
    }

    /// Stage, unstage or revert one change from the diff editor (Revert asks first), then show what's left.
    fn diff_change(&mut self, diff: Entity<crate::diff_view::DiffView>, action: ChangeAction, path: PathBuf, change: bf_core::diff::LineChange, window: &mut Window, cx: &mut Context<Self>) {
        let Some(git) = self.scm.read(cx).git().cloned() else { return };
        let staged = action == ChangeAction::Unstage;
        let untracked = self.scm.read(cx).status().change_for(&path).is_some_and(|c| c.untracked);
        let apply = move |wb: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
            // Stage and Revert work on the file on disk, and the change is one of the text the diff shows:
            // unsaved edits (in the diff, or in the file's tab the diff took its text from) go there first.
            if !staged
                && let Some(ix) = wb.tabs.iter().position(|t| t.save_path(cx).is_some_and(|p| bf_core::paths::same_path(&p, &path)) && t.editor.read(cx).is_modified())
            {
                wb.save_tab(ix, window, cx);
            }
            let (g, p) = (git.clone(), path.clone());
            let task = cx.background_spawn(async move {
                match action {
                    ChangeAction::Stage => g.stage_change(&p, &change)?,
                    ChangeAction::Unstage => g.unstage_change(&p, &change)?,
                    ChangeAction::Revert => g.revert_change(&p, &change)?,
                }
                anyhow::Ok(g.diff_texts_of(&p, staged))
            });
            cx.spawn_in(window, async move |this, cx| {
                let result = task.await;
                let _ = this.update(cx, |wb, cx| match result {
                    Ok((old, new)) => {
                        let kind = if staged { DiffKind::Staged } else { DiffKind::WorkingTree };
                        let file = path.exists().then(|| path.clone());
                        diff.update(cx, |d, cx| d.set_texts(&old, &new, diff_labels(staged, false), file, kind, cx));
                        wb.reload_changed_tabs(cx);
                        wb.scm.update(cx, |s, cx| s.refresh(cx));
                    }
                    Err(e) => {
                        wb.log(format!("Could not change {}: {e:#}", path.display()));
                        wb.show_panel(PanelView::Output, cx);
                    }
                });
            })
            .detach();
        };
        if action != ChangeAction::Revert {
            return apply(self, window, cx);
        }
        let detail = if untracked { "git has no version of this file, so reverting deletes it." } else { "The lines go back to what git has; what you changed in them is lost." };
        let rx = window.prompt(PromptLevel::Warning, "Revert this change?", Some(detail), &["Revert", "Cancel"], cx);
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(0) = rx.await {
                let _ = this.update_in(cx, apply);
            }
        })
        .detach();
    }

    /// A branch switch failed: says why, and when local changes are in the way, offers to stash them.
    fn checkout_failed(&mut self, branch: bf_core::git::Branch, message: String, window: &mut Window, cx: &mut Context<Self>) {
        self.log(format!("Could not switch to {}: {message}", branch.name));
        let in_the_way = message.contains("would be overwritten") || message.contains("Please commit your changes or stash them");
        let detail: String = message.lines().filter(|l| !l.trim().is_empty()).take(12).collect::<Vec<_>>().join("\n");
        let buttons: &[&str] = if in_the_way { &["Stash Changes and Switch", "Cancel"] } else { &["OK"] };
        let question = if in_the_way { format!("Your changes would be overwritten by switching to {}.", branch.name) } else { format!("Could not switch to {}.", branch.name) };
        let rx = window.prompt(PromptLevel::Warning, &question, Some(&detail), buttons, cx);
        cx.spawn_in(window, async move |this, cx| {
            if let (Ok(0), true) = (rx.await, in_the_way) {
                let _ = this.update(cx, |wb, cx| wb.scm.update(cx, |s, cx| s.checkout(branch, true, cx)));
            }
        })
        .detach();
    }

    // ───────────── the merge editor ─────────────

    /// "Merge Conflict: Open Merge Editor": the current file's tab shows its two sides (as git has them)
    /// above the file's editor. Asked again, it goes back to the plain editor.
    pub(super) fn toggle_merge_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.active else { return };
        if matches!(self.tabs[ix].page, Some(Page::Merge(_))) {
            return self.close_merge_editor(ix, window, cx);
        }
        let Some(path) = self.tabs[ix].path.clone().filter(|_| self.tabs[ix].page.is_none()) else {
            self.log("Merge Editor: open the file with the conflict first.");
            return;
        };
        let Some(git) = self.scm.read(cx).git().cloned() else {
            self.log("Merge Editor: the opened folder is not a git repository.");
            return;
        };
        let editor = self.tabs[ix].editor.clone();
        let (git_path, file) = (path.clone(), path);
        let task = cx.background_spawn(async move { git.conflict_sides(&git_path) });
        cx.spawn_in(window, async move |this, cx| {
            let (_, current, incoming) = task.await;
            let _ = this.update_in(cx, |wb, window, cx| {
                let Some(ix) = wb.tabs.iter().position(|t| t.editor == editor) else { return };
                if current.is_none() && incoming.is_none() && editor.read(cx).conflict_summary().0 == 0 {
                    wb.log("Merge Editor: this file has no merge conflict (git has no sides for it and it has no conflict markers).");
                    wb.show_panel(PanelView::Output, cx);
                    return;
                }
                let merge = cx.new(|cx| crate::merge_view::MergeView::new(editor.clone(), file, current, incoming, cx));
                let sub = cx.subscribe_in(&merge, window, |wb: &mut Self, merge, event: &crate::merge_view::MergeEvent, window, cx| {
                    let Some(ix) = wb.tabs.iter().position(|t| matches!(&t.page, Some(Page::Merge(m)) if m == merge)) else { return };
                    match event {
                        crate::merge_view::MergeEvent::Close => wb.close_merge_editor(ix, window, cx),
                        crate::merge_view::MergeEvent::Resolve(path) => wb.resolve_merged_file(ix, path.clone(), window, cx),
                    }
                });
                wb.tabs[ix].page = Some(Page::Merge(merge));
                wb.tabs[ix]._subs.push(sub);
                editor.update(cx, |e, cx| {
                    // Already in one (opened from Ctrl+., or opened again): that's the one to look at.
                    if !e.caret_in_conflict() {
                        e.go_to_conflict(true, cx);
                    }
                    e.focus(window);
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// Back to the file's plain editor.
    fn close_merge_editor(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get_mut(ix) {
            tab.page = None;
            let editor = tab.editor.clone();
            editor.update(cx, |e, _| e.focus(window));
        }
        cx.notify();
    }

    /// "Mark as Resolved": saves the merged file and stages it (which is how git learns the conflict is
    /// settled), then goes back to the plain editor.
    fn resolve_merged_file(&mut self, ix: usize, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.get(ix).is_some_and(|t| t.editor.read(cx).conflict_summary().0 > 0) {
            self.log("There are still conflicts in the file.");
            return;
        }
        self.save_tab(ix, window, cx);
        let Some(git) = self.scm.read(cx).git().cloned() else { return };
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let task = cx.background_spawn(async move { git.stage(std::slice::from_ref(&path)) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |wb, window, cx| {
                match result {
                    Ok(()) => {
                        wb.log(format!("{name} is resolved and staged."));
                        if let Some(ix) = wb.tabs.iter().position(|t| matches!(&t.page, Some(Page::Merge(_))) && t.path.as_deref().is_some_and(|p| p.file_name().is_some_and(|n| n.to_string_lossy() == name))) {
                            wb.close_merge_editor(ix, window, cx);
                        }
                    }
                    Err(e) => {
                        wb.log(format!("Could not stage {name}: {e:#}"));
                        wb.show_panel(PanelView::Output, cx);
                    }
                }
                wb.scm.update(cx, |s, cx| s.refresh(cx));
            });
        })
        .detach();
    }

    // ───────────── history and blame ─────────────

    /// "Git: File History": the commits that changed the current file; picking one opens its diff.
    pub(super) fn open_file_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(git) = self.scm.read(cx).git().cloned() else {
            self.log("The opened folder is not a git repository.");
            return;
        };
        let Some(path) = self.active.and_then(|i| self.tabs.get(i)).and_then(|t| t.path.clone()) else {
            self.log("File History: open a file first.");
            return;
        };
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let task = cx.background_spawn(async move { git.file_history(&path, 500) });
        cx.spawn_in(window, async move |this, cx| {
            let history = task.await;
            let _ = this.update_in(cx, |wb, window, cx| {
                let history = match history {
                    Ok(h) if !h.is_empty() => h,
                    Ok(_) => {
                        wb.log(format!("{name} has no commits yet."));
                        return;
                    }
                    Err(e) => {
                        wb.log(format!("Could not read the history of {name}: {e}"));
                        return;
                    }
                };
                let items = history
                    .into_iter()
                    .map(|(commit, file)| {
                        let what = match file.status {
                            'A' => " (added)",
                            'D' => " (deleted)",
                            'R' => " (renamed)",
                            _ => "",
                        };
                        let mut item = PaletteItem::new(commit.subject.clone(), PaletteAction::Command(Box::new(GitShowFileAtCommit { commit: commit.clone(), file })));
                        item.detail = Some(format!("{} \u{2022} {}, {}{what}", commit.short(), commit.author, commit.when));
                        item.search = Some(format!("{} {} {}", commit.subject, commit.author, commit.short()));
                        item
                    })
                    .collect();
                wb.menu_open = None;
                let palette = cx.new(|cx| Palette::new(Mode::Pick, items, &format!("History of {name} (newest first)"), "", window, cx));
                let sub = cx.subscribe_in(&palette, window, Self::on_palette_event);
                palette.update(cx, |p, cx| p.focus_input(window, cx));
                wb.palette = Some((palette, sub));
                cx.notify();
            });
        })
        .detach();
    }

    /// Opens what `commit` changed in the file that was `filename` (relative) then: "Show Commit of Line".
    pub(super) fn show_commit_of_file(&mut self, commit: String, filename: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(git) = self.scm.read(cx).git().cloned() else { return };
        let task = cx.background_spawn(async move {
            let info = git.commit_info(&commit)?;
            let files = git.commit_files(&info).ok()?;
            let file = files.into_iter().find(|f| git.relative(&f.path) == filename)?;
            Some((info, file))
        });
        cx.spawn_in(window, async move |this, cx| {
            let found = task.await;
            let _ = this.update_in(cx, |wb, window, cx| match found {
                Some((commit, file)) => {
                    wb.log(format!("{} {} \u{2022} {}, {} ({})", commit.short(), commit.subject, commit.author, commit.when, commit.date));
                    wb.open_commit_diff(commit, file, window, cx);
                }
                None => wb.log("Could not find that commit's change to this file."),
            });
        })
        .detach();
    }

    /// "Git: Toggle Line Blame": the annotation after the caret's line, on or off (setting git.blame.currentLine).
    pub(super) fn toggle_blame(&mut self, cx: &mut Context<Self>) {
        let on = !self.settings.get("git.blame.currentLine", true);
        self.settings.set("git.blame.currentLine", json!(on));
        for tab in &self.tabs {
            tab.editor.update(cx, |e, cx| e.set_blame_enabled(on, cx));
        }
        self.log(if on { "Line blame is on." } else { "Line blame is off." });
    }

    pub(super) fn open_branch_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(git) = self.scm.read(cx).git().cloned() else {
            self.log("The opened folder is not a git repository.");
            return;
        };
        let task = cx.background_spawn(async move { git.branches() });
        cx.spawn_in(window, async move |this, cx| {
            let branches = task.await;
            let _ = this.update_in(cx, |wb, window, cx| {
                let branches = match branches {
                    Ok(b) => b,
                    Err(e) => {
                        wb.log(format!("Could not list branches: {e}"));
                        return;
                    }
                };
                let mut items = vec![{
                    let mut item = PaletteItem::new("+ Create new branch…", PaletteAction::Command(Box::new(GitCreateBranch)));
                    item.hint = Some("from HEAD".into());
                    item
                }];
                items.extend(branches.into_iter().map(|b| {
                    let mut item = PaletteItem::new(b.name.clone(), PaletteAction::Branch(b.clone()));
                    item.hint = Some(if b.current { "current".into() } else if b.remote { "remote branch".into() } else { "branch".into() });
                    item
                }));
                wb.menu_open = None;
                let palette = cx.new(|cx| Palette::new(Mode::Branches, items, "Select a branch to check out", "", window, cx));
                let sub = cx.subscribe_in(&palette, window, Self::on_palette_event);
                palette.update(cx, |p, cx| p.focus_input(window, cx));
                wb.palette = Some((palette, sub));
                cx.notify();
            });
        })
        .detach();
    }

    /// Shows `items` in the palette as a pick list titled `placeholder`.
    fn show_pick_list(&mut self, items: Vec<PaletteItem>, placeholder: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_open = None;
        let palette = cx.new(|cx| Palette::new(Mode::Pick, items, placeholder, "", window, cx));
        let sub = cx.subscribe_in(&palette, window, Self::on_palette_event);
        palette.update(cx, |p, cx| p.focus_input(window, cx));
        self.palette = Some((palette, sub));
        cx.notify();
    }

    /// "Pop / Apply / Drop Stash…": the stash entries to pick from (`op`: pop, apply or drop).
    pub(super) fn open_stash_picker(&mut self, op: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(git) = self.scm.read(cx).git().cloned() else {
            self.log("The opened folder is not a git repository.");
            return;
        };
        let task = cx.background_spawn(async move { git.stashes() });
        cx.spawn_in(window, async move |this, cx| {
            let stashes = task.await;
            let _ = this.update_in(cx, |wb, window, cx| {
                let stashes = match stashes {
                    Ok(s) if !s.is_empty() => s,
                    Ok(_) => {
                        wb.log("The stash is empty.");
                        return;
                    }
                    Err(e) => {
                        wb.log(format!("Could not list the stash: {e}"));
                        return;
                    }
                };
                let items = stashes
                    .into_iter()
                    .map(|st| {
                        let action = GitStashEntry { op: op.to_owned(), index: st.index, message: st.message.clone() };
                        let mut item = PaletteItem::new(st.message.clone(), PaletteAction::Command(Box::new(action)));
                        item.detail = Some(format!("stash@{{{}}} \u{2022} {}", st.index, st.when));
                        item
                    })
                    .collect();
                let title = match op {
                    "pop" => "Pick a stash to pop (apply, then remove it)",
                    "apply" => "Pick a stash to apply (it stays on the stash)",
                    _ => "Pick a stash to drop (delete)",
                };
                wb.show_pick_list(items, title, window, cx);
            });
        })
        .detach();
    }

    /// A picked stash entry: pop and apply run right away, drop asks first.
    pub(super) fn stash_entry(&mut self, entry: &GitStashEntry, window: &mut Window, cx: &mut Context<Self>) {
        use bf_core::git::StashOp;
        let (op, index) = match entry.op.as_str() {
            "pop" => (StashOp::Pop, entry.index),
            "apply" => (StashOp::Apply, entry.index),
            _ => (StashOp::Drop, entry.index),
        };
        if op != StashOp::Drop {
            self.scm.update(cx, |s, cx| s.stash_entry(op, index, cx));
            return;
        }
        let rx = window.prompt(PromptLevel::Warning, "Drop this stash?", Some(&format!("\"{}\" is deleted with its changes. This cannot be undone.", entry.message)), &["Drop", "Cancel"], cx);
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(0) = rx.await {
                let _ = this.update(cx, |wb, cx| wb.scm.update(cx, |s, cx| s.stash_entry(op, index, cx)));
            }
        })
        .detach();
    }

    /// "Merge Branch…": the other branches, to merge into the current one.
    pub(super) fn open_merge_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(git) = self.scm.read(cx).git().cloned() else {
            self.log("The opened folder is not a git repository.");
            return;
        };
        let current = self.scm.read(cx).branch().unwrap_or_default().to_owned();
        let task = cx.background_spawn(async move { git.branches() });
        cx.spawn_in(window, async move |this, cx| {
            let branches = task.await;
            let _ = this.update_in(cx, |wb, window, cx| {
                let branches = match branches {
                    Ok(b) => b,
                    Err(e) => {
                        wb.log(format!("Could not list branches: {e}"));
                        return;
                    }
                };
                let items: Vec<PaletteItem> = branches
                    .into_iter()
                    .filter(|b| !b.current)
                    .map(|b| {
                        let mut item = PaletteItem::new(b.name.clone(), PaletteAction::Command(Box::new(GitMergeWith { branch: b.name.clone() })));
                        item.hint = Some(if b.remote { "remote branch".into() } else { "branch".into() });
                        item
                    })
                    .collect();
                if items.is_empty() {
                    wb.log("There is no other branch to merge.");
                    return;
                }
                wb.show_pick_list(items, &format!("Pick a branch to merge into {current}"), window, cx);
            });
        })
        .detach();
    }

    /// "Revert Commit…": the recent commits of the current branch.
    pub(super) fn open_revert_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(git) = self.scm.read(cx).git().cloned() else {
            self.log("The opened folder is not a git repository.");
            return;
        };
        let task = cx.background_spawn(async move { git.log(200, 0) });
        cx.spawn_in(window, async move |this, cx| {
            let log = task.await;
            let _ = this.update_in(cx, |wb, window, cx| {
                let commits = match log {
                    Ok(c) if !c.is_empty() => c,
                    Ok(_) => {
                        wb.log("There are no commits yet.");
                        return;
                    }
                    Err(e) => {
                        wb.log(format!("Could not read the history: {e}"));
                        return;
                    }
                };
                let items = commits
                    .into_iter()
                    .map(|c| {
                        let mut item = PaletteItem::new(c.subject.clone(), PaletteAction::Command(Box::new(GitRevertCommitId { id: c.id.clone(), subject: c.subject.clone() })));
                        item.detail = Some(format!("{} \u{2022} {}, {}", c.short(), c.author, c.when));
                        item.search = Some(format!("{} {} {}", c.subject, c.author, c.short()));
                        item
                    })
                    .collect();
                wb.show_pick_list(items, "Pick a commit to revert (a new commit undoes its changes)", window, cx);
            });
        })
        .detach();
    }

    /// Reverts a commit after asking: a new commit undoes its changes; the history keeps both.
    pub(super) fn revert_commit(&mut self, id: String, subject: String, window: &mut Window, cx: &mut Context<Self>) {
        let detail = "A new commit undoes its changes; the commit itself stays in the history. If later commits changed the same lines, git stops at conflicts to resolve.";
        let rx = window.prompt(PromptLevel::Info, &format!("Revert \"{subject}\"?"), Some(detail), &["Revert", "Cancel"], cx);
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(0) = rx.await {
                let _ = this.update(cx, |wb, cx| wb.scm.update(cx, |s, cx| s.revert_commit(id, cx)));
            }
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workbench::settings_page::view_tests::workbench;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git").args(args).current_dir(dir).output().expect("git");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    /// Picking a branch in the branch picker (what a click on its row does) switches to it.
    #[gpui::test]
    fn picking_a_branch_switches_to_it(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "branches");
        let repo = guard.dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        for (k, v) in [("user.name", "T"), ("user.email", "t@example.com"), ("commit.gpgsign", "false"), ("core.autocrlf", "false")] {
            git(&repo, &["config", k, v]);
        }
        std::fs::write(repo.join("a.txt"), "a\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "first"]);
        git(&repo, &["branch", "feature"]);

        wb.update_in(cx, |wb, window, cx| wb.open_folder(repo.clone(), window, cx));
        cx.run_until_parked();
        wb.update_in(cx, |wb, window, cx| wb.open_branch_picker(window, cx));
        cx.run_until_parked();
        let (palette, branch) = wb.update(cx, |wb, cx| {
            let palette = wb.palette.as_ref().map(|(p, _)| p.clone()).expect("the branch picker is open");
            let branch = palette.read(cx).items_for_test().into_iter().find_map(|a| match a {
                PaletteAction::Branch(b) if b.name == "feature" => Some(b),
                _ => None,
            });
            (palette, branch.expect("feature is listed"))
        });
        palette.update(cx, |_, cx| cx.emit(crate::palette::PaletteEvent::Accept(PaletteAction::Branch(branch))));
        cx.run_until_parked();
        // The switch runs on a background thread; give it a moment.
        for _ in 0..50 {
            if git(&repo, &["branch", "--show-current"]) == "feature" {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
            cx.run_until_parked();
        }
        assert_eq!(git(&repo, &["branch", "--show-current"]), "feature");
        drop(guard);
    }

    /// A real mouse click on the branch's row in the picker switches to it (and closes the picker).
    #[gpui::test]
    fn clicking_a_branch_row_switches_to_it(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "branch-click");
        let repo = guard.dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        for (k, v) in [("user.name", "T"), ("user.email", "t@example.com"), ("commit.gpgsign", "false"), ("core.autocrlf", "false")] {
            git(&repo, &["config", k, v]);
        }
        std::fs::write(repo.join("a.txt"), "a\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "first"]);
        git(&repo, &["branch", "feature"]);

        wb.update_in(cx, |wb, window, cx| wb.open_folder(repo.clone(), window, cx));
        cx.run_until_parked();
        wb.update_in(cx, |wb, window, cx| wb.open_branch_picker(window, cx));
        cx.run_until_parked();
        let row = cx.debug_bounds("palette-row-feature").expect("the feature row is drawn");
        cx.simulate_click(row.center(), gpui::Modifiers::none());
        for _ in 0..50 {
            cx.run_until_parked();
            if git(&repo, &["branch", "--show-current"]) == "feature" {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert_eq!(git(&repo, &["branch", "--show-current"]), "feature");
        assert!(wb.read_with(cx, |wb, _| wb.palette.is_none()), "the picker closes");
        drop(guard);
    }

    /// Local changes in the way: the switch fails with a question instead of silently, and "Stash Changes
    /// and Switch" puts them on the stash and switches.
    #[gpui::test]
    fn a_blocked_switch_asks_and_can_stash(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "blocked-switch");
        let repo = guard.dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        for (k, v) in [("user.name", "T"), ("user.email", "t@example.com"), ("commit.gpgsign", "false"), ("core.autocrlf", "false")] {
            git(&repo, &["config", k, v]);
        }
        std::fs::write(repo.join("a.txt"), "a\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "first"]);
        git(&repo, &["switch", "-qc", "feature"]);
        std::fs::write(repo.join("a.txt"), "feature\n").unwrap();
        git(&repo, &["commit", "-qam", "feature"]);
        git(&repo, &["switch", "-q", "main"]);
        std::fs::write(repo.join("a.txt"), "local edit\n").unwrap();

        wb.update_in(cx, |wb, window, cx| wb.open_folder(repo.clone(), window, cx));
        cx.run_until_parked();
        let feature = bf_core::git::Branch { name: "feature".into(), current: false, remote: false };
        wb.update(cx, |wb, cx| wb.scm.update(cx, |s, cx| s.checkout(feature, false, cx)));
        let mut asked = false;
        for _ in 0..50 {
            cx.run_until_parked();
            if cx.has_pending_prompt() {
                asked = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(asked, "the failed switch shows a dialog");
        assert_eq!(git(&repo, &["branch", "--show-current"]), "main");
        cx.simulate_prompt_answer("Stash Changes and Switch");
        for _ in 0..50 {
            cx.run_until_parked();
            if git(&repo, &["branch", "--show-current"]) == "feature" {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert_eq!(git(&repo, &["branch", "--show-current"]), "feature");
        assert!(git(&repo, &["stash", "list"]).contains("ByteForge: before switching to feature"));
        drop(guard);
    }

    /// The diff editor stages one change of two (the other stays unstaged, the file untouched) and
    /// collapses the unchanged lines.
    #[gpui::test]
    fn the_diff_editor_stages_one_change(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "stage-change");
        let repo = guard.dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        for (k, v) in [("user.name", "T"), ("user.email", "t@example.com"), ("commit.gpgsign", "false"), ("core.autocrlf", "false")] {
            git(&repo, &["config", k, v]);
        }
        let file = repo.join("Big.cs");
        let lines: Vec<String> = (1..=40).map(|i| format!("line {i}")).collect();
        std::fs::write(&file, lines.join("\n") + "\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "first"]);
        let mut edited = lines.clone();
        edited[2] = "line three".into();
        edited[35] = "line thirty-six".into();
        std::fs::write(&file, edited.join("\n") + "\n").unwrap();

        wb.update_in(cx, |wb, window, cx| wb.open_folder(repo.clone(), window, cx));
        cx.run_until_parked();
        let change = bf_core::git::FileChange { path: file.clone(), orig: None, index: '.', worktree: 'M', untracked: false, conflict: false };
        wb.update_in(cx, |wb, window, cx| wb.open_diff(change, false, window, cx));
        let mut diff = None;
        for _ in 0..100 {
            cx.run_until_parked();
            diff = wb.update(cx, |wb, _| wb.tabs.iter().find_map(|t| match &t.page {
                Some(Page::Diff(d)) => Some(d.clone()),
                _ => None,
            }));
            if diff.as_ref().is_some_and(|d| d.read_with(cx, |d, cx| d.state_for_test(cx).0) == 2) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let diff = diff.expect("the diff tab opened");
        assert_eq!(diff.read_with(cx, |d, cx| d.state_for_test(cx)), (2, Some(0), 0), "two changes, on the first, nothing folded");

        // Collapse Unchanged hides the long stretch between the two changes.
        diff.update(cx, |d, cx| d.toggle_collapsed(cx));
        assert!(diff.read_with(cx, |d, cx| d.state_for_test(cx).2) > 20);

        // Stage Change on the first change.
        diff.update(cx, |d, cx| d.change_action(ChangeAction::Stage, cx));
        for _ in 0..100 {
            cx.run_until_parked();
            if diff.read_with(cx, |d, cx| d.state_for_test(cx).0) == 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(diff.read_with(cx, |d, cx| d.state_for_test(cx).0), 1, "one unstaged change left in the diff");
        let staged = git(&repo, &["diff", "--cached", "--unified=0"]);
        assert!(staged.contains("+line three") && !staged.contains("thirty-six"), "{staged}");
        let unstaged = git(&repo, &["diff", "--unified=0"]);
        assert!(unstaged.contains("+line thirty-six") && !unstaged.contains("line three"), "{unstaged}");
        drop(guard);
    }

    /// The working-tree side of a diff is the file's own text (spacer rows keep the sides aligned) and can
    /// be edited: the diff follows, the file's open tab gets the edit, and saving the diff tab writes it.
    #[gpui::test]
    fn the_diff_editor_edits_the_working_tree(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "diff-edit");
        let repo = guard.dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        for (k, v) in [("user.name", "T"), ("user.email", "t@example.com"), ("commit.gpgsign", "false"), ("core.autocrlf", "false")] {
            git(&repo, &["config", k, v]);
        }
        let file = repo.join("A.cs");
        std::fs::write(&file, "one\ntwo\nthree\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "first"]);
        std::fs::write(&file, "one\nTWO\nthree\nfour\n").unwrap();

        wb.update_in(cx, |wb, window, cx| {
            wb.open_folder(repo.clone(), window, cx);
            wb.open_file(file.clone(), None, window, cx);
        });
        cx.run_until_parked();
        let file_editor = wb.update(cx, |wb, _| wb.active_editor().unwrap());
        let change = bf_core::git::FileChange { path: file.clone(), orig: None, index: '.', worktree: 'M', untracked: false, conflict: false };
        wb.update_in(cx, |wb, window, cx| wb.open_diff(change, false, window, cx));
        let mut found = None;
        for _ in 0..100 {
            cx.run_until_parked();
            found = wb.update(cx, |wb, _| {
                wb.tabs.iter().enumerate().find_map(|(ix, t)| match &t.page {
                    Some(Page::Diff(d)) => Some((ix, d.clone(), t.editor.clone())),
                    _ => None,
                })
            });
            if found.as_ref().is_some_and(|(_, d, _)| d.read_with(cx, |d, cx| d.state_for_test(cx).0) == 2) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let (ix, diff, right) = found.expect("the diff tab opened");
        assert_eq!(right.read_with(cx, |e, _| e.state.buffer.text()), "one\nTWO\nthree\nfour\n", "the file's own text, no filler lines");
        assert_eq!(diff.read_with(cx, |d, cx| d.spacer_rows_for_test(cx)), (1, 0), "one spacer on the left, across from \"four\"");
        assert!(!right.read_with(cx, |e, _| e.read_only));

        // Undo the first change by typing on the right: one change left, the open tab has the edit.
        right.update(cx, |e, cx| e.replace_text_minimal("one\ntwo\nthree\nfour\n", cx));
        cx.run_until_parked();
        assert_eq!(diff.read_with(cx, |d, cx| d.state_for_test(cx).0), 1);
        assert_eq!(file_editor.read_with(cx, |e, _| e.state.buffer.text()), "one\ntwo\nthree\nfour\n");

        // Ctrl+S on the diff tab writes the file; both views are saved.
        wb.update_in(cx, |wb, window, cx| wb.save_tab(ix, window, cx));
        cx.run_until_parked();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "one\ntwo\nthree\nfour\n");
        assert!(!right.read_with(cx, |e, _| e.is_modified()));
        assert!(!file_editor.read_with(cx, |e, _| e.is_modified()));

        // Inline is read-only; back side by side the right is editable again.
        wb.update_in(cx, |_, window, cx| diff.update(cx, |d, cx| d.toggle_inline(window, cx)));
        assert!(right.read_with(cx, |e, _| e.read_only));
        wb.update_in(cx, |_, window, cx| diff.update(cx, |d, cx| d.toggle_inline(window, cx)));
        assert!(!right.read_with(cx, |e, _| e.read_only));
        assert_eq!(right.read_with(cx, |e, _| e.state.buffer.text()), "one\ntwo\nthree\nfour\n");
        drop(guard);
    }

    /// The blame gutter: a column with each run of lines' commit and author, drawn left of the line numbers.
    #[gpui::test]
    fn the_blame_gutter_shows_each_commit(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "blame-gutter");
        let repo = guard.dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        for (k, v) in [("user.name", "Ann Example"), ("user.email", "t@example.com"), ("commit.gpgsign", "false")] {
            git(&repo, &["config", k, v]);
        }
        let file = repo.join("A.cs");
        std::fs::write(&file, "one\ntwo\nthree\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "first"]);
        std::fs::write(&file, "one\ntwo\nthree\nfour\n").unwrap();
        git(&repo, &["commit", "-qam", "second"]);

        wb.update_in(cx, |wb, window, cx| {
            wb.open_folder(repo.clone(), window, cx);
            wb.open_file(file.clone(), None, window, cx);
        });
        cx.run_until_parked();
        let editor = wb.update(cx, |wb, _| wb.active_editor().expect("the file is open"));
        // The SCM view finds the repository in the background; the editor gets it with its git base.
        for _ in 0..100 {
            cx.run_until_parked();
            wb.update(cx, |wb, cx| wb.update_git_bases(cx));
            editor.update(cx, |e, cx| e.set_blame_gutter(true, cx));
            cx.run_until_parked();
            if editor.read_with(cx, |e, _| e.blame_gutter() && e.blame_gutter_label(0, 0, 0).is_some()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        cx.run_until_parked();
        editor.read_with(cx, |e, _| {
            assert!(e.blame_gutter());
            let label = |line| e.blame_gutter_label(line, 0, i64::MAX / 4);
            let first = label(0).expect("line 1 is blamed");
            assert!(first.contains("Ann Example"), "{first}");
            assert_eq!(label(1), None, "line 2 continues line 1's commit");
            assert!(label(3).is_some_and(|l| l != first), "line 4 is from the second commit");
            assert!(e.layout.blame > 0.0, "the gutter has the column after a paint");
        });
        drop(guard);
    }

    /// The merge editor on a real conflict: both sides show, a side is accepted in the result, and Mark
    /// as Resolved saves and stages the file, which settles the conflict for git.
    #[gpui::test]
    fn the_merge_editor_resolves_a_conflict(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "merge-editor");
        let repo = guard.dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        for (k, v) in [("user.name", "T"), ("user.email", "t@example.com"), ("commit.gpgsign", "false"), ("core.autocrlf", "false")] {
            git(&repo, &["config", k, v]);
        }
        let file = repo.join("A.cs");
        std::fs::write(&file, "class A { }\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "first"]);
        git(&repo, &["switch", "-qc", "feature"]);
        std::fs::write(&file, "class A { int incoming; }\n").unwrap();
        git(&repo, &["commit", "-qam", "incoming"]);
        git(&repo, &["switch", "-q", "main"]);
        std::fs::write(&file, "class A { int current; }\n").unwrap();
        git(&repo, &["commit", "-qam", "current"]);
        assert!(!std::process::Command::new("git").args(["merge", "feature"]).current_dir(&repo).output().unwrap().status.success());

        wb.update_in(cx, |wb, window, cx| {
            wb.open_folder(repo.clone(), window, cx);
            wb.open_file(file.clone(), None, window, cx);
        });
        let merge_of = |wb: &Workbench| wb.tabs.iter().find_map(|t| match &t.page {
            Some(Page::Merge(m)) => Some(m.clone()),
            _ => None,
        });
        let mut merge = None;
        for _ in 0..100 {
            cx.run_until_parked();
            if wb.update(cx, |wb, cx| wb.scm.read(cx).git().is_some()) && merge.is_none() {
                wb.update_in(cx, |wb, window, cx| {
                    if merge_of(wb).is_none() {
                        wb.toggle_merge_editor(window, cx);
                    }
                });
            }
            cx.run_until_parked();
            merge = wb.update(cx, |wb, _| merge_of(wb));
            if merge.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        let merge = merge.expect("the merge editor opened");
        cx.run_until_parked();
        assert_eq!(merge.read_with(cx, |m, cx| m.sides_for_test(cx)), ("class A { int current; }\n".to_owned(), "class A { int incoming; }\n".to_owned()));
        assert_eq!(merge.read_with(cx, |m, cx| m.conflicts_left(cx)), 1);
        // The tab is still the file's tab: its editor is the result.
        let editor = wb.update(cx, |wb, _| wb.active_editor().unwrap());
        editor.update(cx, |e, cx| e.accept_conflict(bf_core::conflicts::Resolution::Current, cx));
        assert_eq!(merge.read_with(cx, |m, cx| m.conflicts_left(cx)), 0);
        merge.update(cx, |_, cx| cx.emit(crate::merge_view::MergeEvent::Resolve(file.clone())));
        for _ in 0..100 {
            cx.run_until_parked();
            if wb.update(cx, |wb, _| merge_of(wb).is_none()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        assert!(wb.update(cx, |wb, _| merge_of(wb).is_none()), "back to the plain editor");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "class A { int current; }\n");
        assert_eq!(git(&repo, &["diff", "--name-only", "--diff-filter=U"]), "", "git has no unmerged file left");
        drop(guard);
    }

    // ───────────── click-through: what a user does with the mouse and keys ─────────────

    /// A repository in the test's folder with one commit of `files`.
    fn repo_with(guard: &crate::workbench::settings_page::view_tests::TestWorkbench, files: &[(&str, &str)]) -> PathBuf {
        let repo = guard.dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        for (k, v) in [("user.name", "Ann Example"), ("user.email", "t@example.com"), ("commit.gpgsign", "false"), ("core.autocrlf", "false")] {
            git(&repo, &["config", k, v]);
        }
        for (name, text) in files {
            std::fs::write(repo.join(name), text).unwrap();
        }
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "first"]);
        repo
    }

    /// Runs the app until `done` holds (git runs on background threads), or panics with `what`.
    fn wait_for(cx: &mut gpui::VisualTestContext, what: &str, mut done: impl FnMut(&mut gpui::VisualTestContext) -> bool) {
        for _ in 0..200 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("timed out waiting for: {what}");
    }

    /// Clicks the element drawn with `selector` (it has to be on screen).
    fn click(cx: &mut gpui::VisualTestContext, selector: &'static str) {
        cx.run_until_parked();
        let bounds = cx.debug_bounds(selector).unwrap_or_else(|| panic!("{selector} is not on screen"));
        cx.simulate_click(bounds.center(), gpui::Modifiers::none());
        cx.run_until_parked();
    }

    /// Clicks at `(x, y)` in window coordinates.
    fn click_at(cx: &mut gpui::VisualTestContext, x: f32, y: f32) {
        cx.simulate_click(gpui::point(px(x), px(y)), gpui::Modifiers::none());
        cx.run_until_parked();
    }

    fn diff_tab(wb: &Entity<Workbench>, cx: &mut gpui::VisualTestContext) -> Option<(usize, Entity<crate::diff_view::DiffView>, Entity<EditorView>)> {
        wb.update(cx, |wb, _| {
            wb.tabs.iter().enumerate().find_map(|(ix, t)| match &t.page {
                Some(Page::Diff(d)) => Some((ix, d.clone(), t.editor.clone())),
                _ => None,
            })
        })
    }

    /// Opens `file` and waits until source control knows the repository and the file's change.
    fn open_with_status(wb: &Entity<Workbench>, cx: &mut gpui::VisualTestContext, repo: &Path, file: &Path) {
        wb.update_in(cx, |wb, window, cx| {
            wb.open_folder(repo.to_path_buf(), window, cx);
            wb.open_file(file.to_path_buf(), None, window, cx);
        });
        wait_for(cx, "git status", |cx| wb.update(cx, |wb, cx| wb.scm.read(cx).status().change_for(file).is_some()));
    }

    /// The diff editor's toolbar, clicked through: Next Change, Collapse Unchanged, Stage Change, Revert
    /// Change (with its question), Inline / Side by Side and Open File.
    #[gpui::test]
    fn clicking_through_the_diff_toolbar(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "diff-clicks");
        let lines: Vec<String> = (1..=40).map(|i| format!("line {i}")).collect();
        let repo = repo_with(&guard, &[("Big.cs", &(lines.join("\n") + "\n"))]);
        let file = repo.join("Big.cs");
        let mut edited = lines.clone();
        edited[2] = "line three".into();
        edited[35] = "line thirty-six".into();
        std::fs::write(&file, edited.join("\n") + "\n").unwrap();
        open_with_status(&wb, cx, &repo, &file);

        // "Git: Open Changes" on the file's tab.
        cx.dispatch_action(crate::actions::GitOpenChanges);
        wait_for(cx, "the diff tab", |cx| diff_tab(&wb, cx).is_some_and(|(_, d, _)| d.read_with(cx, |d, cx| d.state_for_test(cx).0) == 2));
        let (_, diff, right) = diff_tab(&wb, cx).unwrap();
        assert_eq!(diff.read_with(cx, |d, cx| d.state_for_test(cx).1), Some(0));

        click(cx, "diff-next");
        assert_eq!(diff.read_with(cx, |d, cx| d.state_for_test(cx).1), Some(1), "Next Change goes to the second change");
        assert_eq!(right.read_with(cx, |e, _| e.state.caret().line), 35);
        click(cx, "diff-prev");
        assert_eq!(diff.read_with(cx, |d, cx| d.state_for_test(cx).1), Some(0), "Previous Change goes back");

        click(cx, "diff-collapse");
        assert!(diff.read_with(cx, |d, cx| d.state_for_test(cx).2) > 20, "Collapse Unchanged folds the middle");
        click(cx, "diff-collapse");
        assert_eq!(diff.read_with(cx, |d, cx| d.state_for_test(cx).2), 0, "Show All unfolds it");

        // Stage the second change.
        click(cx, "diff-next");
        click(cx, "diff-stage");
        wait_for(cx, "the staged change", |cx| diff.read_with(cx, |d, cx| d.state_for_test(cx).0) == 1);
        let staged = git(&repo, &["diff", "--cached", "--unified=0"]);
        assert!(staged.contains("+line thirty-six") && !staged.contains("line three"), "{staged}");

        // Revert the first: it asks, and Cancel leaves the file alone.
        click(cx, "diff-revert");
        assert!(cx.has_pending_prompt(), "Revert asks first");
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        assert!(std::fs::read_to_string(&file).unwrap().contains("line three"));
        click(cx, "diff-revert");
        cx.simulate_prompt_answer("Revert");
        wait_for(cx, "the reverted change", |cx| diff.read_with(cx, |d, cx| d.state_for_test(cx).0) == 0);
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("line 3\n") && text.contains("line thirty-six"), "{text}");
        assert_eq!(diff.read_with(cx, |d, cx| d.state_for_test(cx).1), None, "no change, no Stage Change");

        // Inline and back.
        click(cx, "diff-mode");
        assert!(right.read_with(cx, |e, _| e.read_only), "inline is read-only");
        click(cx, "diff-mode");
        assert!(!right.read_with(cx, |e, _| e.read_only));

        // Open File: the file's own tab.
        click(cx, "diff-open");
        let active_is_file = wb.update(cx, |wb, _| wb.active.and_then(|i| wb.tabs.get(i)).is_some_and(|t| t.page.is_none() && t.path.as_deref() == Some(file.as_path())));
        assert!(active_is_file, "Open File shows the file's tab");
        drop(guard);
    }

    /// The diff shows the file's open tab with its unsaved edits; Stage Change on one of those saves the
    /// tab first, so the change staged is the one shown.
    #[gpui::test]
    fn staging_an_unsaved_change_saves_it_first(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "diff-unsaved");
        let lines: Vec<String> = (1..=20).map(|i| format!("line {i}")).collect();
        let repo = repo_with(&guard, &[("A.cs", &(lines.join("\n") + "\n"))]);
        let file = repo.join("A.cs");
        let mut on_disk = lines.clone();
        on_disk[17] = "line eighteen".into();
        std::fs::write(&file, on_disk.join("\n") + "\n").unwrap();
        open_with_status(&wb, cx, &repo, &file);
        let mut typed = on_disk.clone();
        typed[2] = "line three".into();
        let doc = wb.update(cx, |wb, _| wb.active_editor().unwrap());
        doc.update(cx, |e, cx| e.replace_text_minimal(&(typed.join("\n") + "\n"), cx));

        cx.dispatch_action(crate::actions::GitOpenChanges);
        wait_for(cx, "the diff tab", |cx| diff_tab(&wb, cx).is_some_and(|(_, d, _)| d.read_with(cx, |d, cx| d.state_for_test(cx).0) == 2));
        let (_, diff, _) = diff_tab(&wb, cx).unwrap();
        assert_eq!(diff.read_with(cx, |d, cx| d.state_for_test(cx).1), Some(0), "on the unsaved change");
        click(cx, "diff-stage");
        wait_for(cx, "the staged change", |cx| diff.read_with(cx, |d, cx| d.state_for_test(cx).0) == 1);
        let staged = git(&repo, &["diff", "--cached", "--unified=0"]);
        assert!(staged.contains("+line three") && !staged.contains("eighteen"), "{staged}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), typed.join("\n") + "\n");
        drop(guard);
    }

    /// Two changed files of the same name get a diff tab each: reusing one would show the second file
    /// with the first one's unsaved edits, and Ctrl+S would write them into the wrong file.
    #[gpui::test]
    fn same_named_files_get_their_own_diff_tabs(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "diff-same-name");
        let repo = guard.dir.join("repo");
        std::fs::create_dir_all(repo.join("a")).unwrap();
        std::fs::create_dir_all(repo.join("b")).unwrap();
        std::fs::write(repo.join("a/P.cs"), "a\n").unwrap();
        std::fs::write(repo.join("b/P.cs"), "b\n").unwrap();
        let repo = repo_with(&guard, &[]);
        let (a, b) = (repo.join("a/P.cs"), repo.join("b/P.cs"));
        std::fs::write(&a, "a changed\n").unwrap();
        std::fs::write(&b, "b changed\n").unwrap();
        open_with_status(&wb, cx, &repo, &a);
        let change = |p: &Path| bf_core::git::FileChange { path: p.to_path_buf(), orig: None, index: '.', worktree: 'M', untracked: false, conflict: false };
        wb.update_in(cx, |wb, window, cx| wb.open_diff(change(&a), false, window, cx));
        wait_for(cx, "a's diff", |cx| diff_tab(&wb, cx).is_some());
        let (_, _, right) = diff_tab(&wb, cx).unwrap();
        right.update(cx, |e, cx| e.replace_text_minimal("a edited\n", cx));
        wb.update_in(cx, |wb, window, cx| wb.open_diff(change(&b), false, window, cx));
        let diffs = |wb: &Entity<Workbench>, cx: &mut gpui::VisualTestContext| wb.update(cx, |wb, _| wb.tabs.iter().filter(|t| matches!(t.page, Some(Page::Diff(_)))).count());
        wait_for(cx, "b's diff", |cx| diffs(&wb, cx) == 2);
        assert_eq!(right.read_with(cx, |e, _| e.state.buffer.text()), "a edited\n", "a's tab keeps its edit");
        drop(guard);
    }

    /// The staged diff: Unstage Change on one of two staged changes.
    #[gpui::test]
    fn clicking_unstage_change(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "diff-unstage");
        let lines: Vec<String> = (1..=20).map(|i| format!("line {i}")).collect();
        let repo = repo_with(&guard, &[("A.cs", &(lines.join("\n") + "\n"))]);
        let file = repo.join("A.cs");
        let mut edited = lines.clone();
        edited[1] = "line two".into();
        edited[17] = "line eighteen".into();
        std::fs::write(&file, edited.join("\n") + "\n").unwrap();
        git(&repo, &["add", "."]);
        open_with_status(&wb, cx, &repo, &file);

        cx.dispatch_action(crate::actions::GitOpenChanges);
        wait_for(cx, "the staged diff", |cx| diff_tab(&wb, cx).is_some_and(|(_, d, _)| d.read_with(cx, |d, cx| d.state_for_test(cx).0) == 2));
        let (_, diff, _) = diff_tab(&wb, cx).unwrap();
        assert!(cx.debug_bounds("diff-stage").is_none() && cx.debug_bounds("diff-revert").is_none(), "a staged diff only unstages");
        click(cx, "diff-unstage");
        wait_for(cx, "the unstaged change", |cx| diff.read_with(cx, |d, cx| d.state_for_test(cx).0) == 1);
        let staged = git(&repo, &["diff", "--cached", "--unified=0"]);
        assert!(staged.contains("+line eighteen") && !staged.contains("line two"), "{staged}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), edited.join("\n") + "\n", "the file keeps both changes");
        drop(guard);
    }

    /// Typing into the working-tree side of the diff and saving it with Ctrl+S keeps the file's CRLF
    /// line endings.
    #[gpui::test]
    fn typing_in_the_diff_and_saving_keeps_crlf(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "diff-crlf");
        let repo = repo_with(&guard, &[("A.cs", "one\r\ntwo\r\nthree\r\n")]);
        let file = repo.join("A.cs");
        std::fs::write(&file, "one\r\nTWO\r\nthree\r\n").unwrap();
        open_with_status(&wb, cx, &repo, &file);
        // Only the diff tab: the file's tab would otherwise save it with its own line endings.
        wb.update_in(cx, |wb, window, cx| wb.close_tab(0, window, cx));
        let change = wb.update(cx, |wb, cx| wb.scm.read(cx).status().change_for(&file).cloned().unwrap());
        wb.update_in(cx, |wb, window, cx| wb.open_diff(change, false, window, cx));
        wait_for(cx, "the diff tab", |cx| diff_tab(&wb, cx).is_some_and(|(_, d, _)| d.read_with(cx, |d, cx| d.state_for_test(cx).0) == 1));
        let (_, _, right) = diff_tab(&wb, cx).unwrap();

        // Click at the end of line 3 on the right and type.
        let (x, y) = right.read_with(cx, |e, _| (e.layout.text_left + 20.0 * e.layout.char_width, e.layout.top + 2.5 * e.layout.line_height));
        click_at(cx, x, y);
        assert_eq!(right.read_with(cx, |e, _| e.state.caret().line), 2);
        cx.simulate_input("!");
        cx.simulate_keystrokes("ctrl-s");
        cx.run_until_parked();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "one\r\nTWO\r\nthree!\r\n");
        drop(guard);
    }

    /// "Git: Toggle Blame Gutter", then a click on a gutter entry opens that commit's change to the file.
    #[gpui::test]
    fn clicking_a_blame_gutter_entry_opens_its_commit(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "blame-click");
        let repo = repo_with(&guard, &[("A.cs", "one\ntwo\n")]);
        let file = repo.join("A.cs");
        std::fs::write(&file, "one\ntwo\nthree\n").unwrap();
        git(&repo, &["commit", "-qam", "add three"]);
        std::fs::write(&file, "one\ntwo\nthree\nfour\n").unwrap();
        wb.update_in(cx, |wb, window, cx| {
            wb.open_folder(repo.clone(), window, cx);
            wb.open_file(file.clone(), None, window, cx);
        });
        let editor = wb.update(cx, |wb, _| wb.active_editor().unwrap());
        wait_for(cx, "the repository", |cx| wb.update(cx, |wb, cx| wb.scm.read(cx).git().is_some()));
        wb.update(cx, |wb, cx| wb.update_git_bases(cx));
        cx.dispatch_action(crate::actions::GitToggleBlameGutter);
        wait_for(cx, "the blame column", |cx| editor.read_with(cx, |e, _| e.layout.blame > 0.0 && e.blame_gutter_label(2, 0, 0).is_some()));

        // Line 3's entry: the "add three" commit.
        let (x, y) = editor.read_with(cx, |e, _| (e.layout.left + e.layout.glyph + 10.0, e.layout.top + 2.5 * e.layout.line_height));
        click_at(cx, x, y);
        wait_for(cx, "the commit's diff", |cx| diff_tab(&wb, cx).is_some());
        let title = wb.update(cx, |wb, _| wb.active.map(|i| wb.tabs[i].title.clone()));
        let short = git(&repo, &["rev-parse", "--short=7", "HEAD"]);
        assert_eq!(title, Some(format!("A.cs ({short})")));

        // Line 4 isn't committed: no tab.
        wb.update_in(cx, |wb, window, cx| {
            let ix = wb.tabs.iter().position(|t| t.editor == editor).unwrap();
            wb.activate(ix, window, cx);
        });
        cx.run_until_parked();
        let tabs = wb.update(cx, |wb, _| wb.tabs.len());
        let (x, y) = editor.read_with(cx, |e, _| (e.layout.left + e.layout.glyph + 10.0, e.layout.top + 3.5 * e.layout.line_height));
        click_at(cx, x, y);
        for _ in 0..10 {
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(wb.update(cx, |wb, _| wb.tabs.len()), tabs);

        // Toggled again, the column goes.
        cx.dispatch_action(crate::actions::GitToggleBlameGutter);
        cx.run_until_parked();
        assert_eq!(editor.read_with(cx, |e, _| e.layout.blame), 0.0);
        drop(guard);
    }

    /// The line blame after the caret's line, and "Git: Toggle Line Blame" turning it off and on.
    #[gpui::test]
    fn toggling_line_blame(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "line-blame");
        let repo = repo_with(&guard, &[("A.cs", "one\ntwo\n")]);
        let file = repo.join("A.cs");
        wb.update_in(cx, |wb, window, cx| {
            wb.open_folder(repo.clone(), window, cx);
            wb.open_file(file.clone(), None, window, cx);
        });
        let editor = wb.update(cx, |wb, _| wb.active_editor().unwrap());
        wait_for(cx, "the repository", |cx| wb.update(cx, |wb, cx| wb.scm.read(cx).git().is_some()));
        wb.update(cx, |wb, cx| wb.update_git_bases(cx));
        wait_for(cx, "the line blame", |cx| editor.read_with(cx, |e, _| e.end_of_line_note(0).is_some()));
        assert!(editor.read_with(cx, |e, _| e.end_of_line_note(0).unwrap().contains("Ann Example")));
        cx.dispatch_action(crate::actions::GitToggleBlame);
        cx.run_until_parked();
        assert_eq!(editor.read_with(cx, |e, _| e.end_of_line_note(0)), None);
        cx.dispatch_action(crate::actions::GitToggleBlame);
        wait_for(cx, "the line blame again", |cx| editor.read_with(cx, |e, _| e.end_of_line_note(0).is_some()));
        drop(guard);
    }

    /// Two conflicting commits on `A.cs`, merged: a repository with two conflicts in the file.
    fn conflicted_repo(guard: &crate::workbench::settings_page::view_tests::TestWorkbench) -> PathBuf {
        let base: String = (1..=12).map(|i| format!("line {i}\n")).collect();
        let repo = repo_with(guard, &[("A.cs", &base)]);
        let file = repo.join("A.cs");
        let with = |a: &str, b: &str| base.replace("line 2\n", &format!("{a}\n")).replace("line 11\n", &format!("{b}\n"));
        git(&repo, &["switch", "-qc", "feature"]);
        std::fs::write(&file, with("theirs 2", "theirs 11")).unwrap();
        git(&repo, &["commit", "-qam", "incoming"]);
        git(&repo, &["switch", "-q", "main"]);
        std::fs::write(&file, with("ours 2", "ours 11")).unwrap();
        git(&repo, &["commit", "-qam", "current"]);
        assert!(!std::process::Command::new("git").args(["merge", "feature"]).current_dir(&repo).output().unwrap().status.success());
        repo
    }

    /// Inline: Ctrl+. in a conflict offers Accept Current / Incoming / Both, and so does the note after its
    /// first line, which can be clicked.
    #[gpui::test]
    fn accepting_a_conflict_inline(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "conflict-inline");
        let repo = conflicted_repo(&guard);
        let file = repo.join("A.cs");
        wb.update_in(cx, |wb, window, cx| {
            wb.open_folder(repo.clone(), window, cx);
            wb.open_file(file.clone(), None, window, cx);
        });
        cx.run_until_parked();
        let editor = wb.update(cx, |wb, _| wb.active_editor().unwrap());
        assert_eq!(editor.read_with(cx, |e, _| e.conflict_summary().0), 2);
        assert!(editor.read_with(cx, |e, _| e.end_of_line_note(1).is_some_and(|n| n.contains("Accept Current"))));

        // Click into the first conflict's incoming side, Ctrl+., and take the second entry (Incoming).
        let (x, y) = editor.read_with(cx, |e, _| (e.layout.text_left + 4.0, e.layout.top + 3.5 * e.layout.line_height));
        click_at(cx, x, y);
        cx.simulate_keystrokes("ctrl-. down enter");
        cx.run_until_parked();
        assert_eq!(editor.read_with(cx, |e, _| e.conflict_summary().0), 1);
        let text = editor.read_with(cx, |e, _| e.state.buffer.text());
        assert!(text.starts_with("line 1\ntheirs 2\nline 3\n"), "{text}");

        // A click on "Accept Both" after the second conflict's first line.
        let (x, y) = editor.read_with(cx, |e, _| e.conflict_note_point_for_test(bf_core::conflicts::Resolution::Both)).expect("the note is drawn");
        click_at(cx, x, y);
        let text = editor.read_with(cx, |e, _| e.state.buffer.text());
        assert!(text.ends_with("line 10\nours 11\ntheirs 11\nline 12\n"), "{text}");
        assert_eq!(editor.read_with(cx, |e, _| e.conflict_summary().0), 0);
        drop(guard);
    }

    /// The merge editor clicked through: Close, Next Conflict, Accept Incoming, Accept Current with the
    /// caret outside any conflict, Mark as Resolved.
    #[gpui::test]
    fn clicking_through_the_merge_editor(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "merge-clicks");
        let repo = conflicted_repo(&guard);
        let file = repo.join("A.cs");
        wb.update_in(cx, |wb, window, cx| {
            wb.open_folder(repo.clone(), window, cx);
            wb.open_file(file.clone(), None, window, cx);
        });
        let editor = wb.update(cx, |wb, _| wb.active_editor().unwrap());
        wait_for(cx, "the repository", |cx| wb.update(cx, |wb, cx| wb.scm.read(cx).git().is_some()));
        let merge_open = |wb: &Entity<Workbench>, cx: &mut gpui::VisualTestContext| wb.update(cx, |wb, _| wb.tabs.iter().any(|t| matches!(t.page, Some(Page::Merge(_)))));

        cx.dispatch_action(crate::actions::ToggleMergeEditor);
        wait_for(cx, "the merge editor", |cx| merge_open(&wb, cx));
        click(cx, "merge-close");
        assert!(!merge_open(&wb, cx), "Close Merge Editor goes back to the plain editor");
        cx.dispatch_action(crate::actions::ToggleMergeEditor);
        wait_for(cx, "the merge editor again", |cx| merge_open(&wb, cx));
        assert_eq!(editor.read_with(cx, |e, _| e.state.caret().line), 1, "on the first conflict");

        click(cx, "merge-next");
        assert!(editor.read_with(cx, |e, _| e.state.caret().line) > 5, "on the second conflict");
        click(cx, "merge-incoming");
        assert_eq!(editor.read_with(cx, |e, _| e.conflict_summary().0), 1);

        // The caret on line 1, outside the conflict left: the button still resolves it.
        editor.update(cx, |e, cx| e.go_to(bf_text::Point::new(0, 0), cx));
        click(cx, "merge-current");
        assert_eq!(editor.read_with(cx, |e, _| e.conflict_summary().0), 0, "Accept Current takes the next conflict");
        let expected: String = (1..=12)
            .map(|i| match i {
                2 => "ours 2\n".to_owned(),
                11 => "theirs 11\n".to_owned(),
                _ => format!("line {i}\n"),
            })
            .collect();
        assert_eq!(editor.read_with(cx, |e, _| e.state.buffer.text()), expected);

        click(cx, "merge-resolve");
        wait_for(cx, "the resolved file", |cx| !merge_open(&wb, cx));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), expected);
        assert_eq!(git(&repo, &["diff", "--name-only", "--diff-filter=U"]), "");
        drop(guard);
    }

    // ───────────── the "…" menu: undo, stash, merge, tags, revert ─────────────

    /// A repository with two commits of `A.cs`, opened, with the Source Control view showing.
    fn two_commits(guard: &crate::workbench::settings_page::view_tests::TestWorkbench, wb: &Entity<Workbench>, cx: &mut gpui::VisualTestContext) -> PathBuf {
        let repo = repo_with(guard, &[("A.cs", "one\n")]);
        std::fs::write(repo.join("A.cs"), "two\n").unwrap();
        git(&repo, &["commit", "-qam", "second commit\n\nwith a body"]);
        wb.update_in(cx, |wb, window, cx| {
            wb.open_folder(repo.clone(), window, cx);
            wb.show_side(SideView::SourceControl, cx);
        });
        wait_for(cx, "git status", |cx| wb.update(cx, |wb, cx| wb.scm.read(cx).status().has_commits));
        repo
    }

    fn commit_count(repo: &Path) -> usize {
        git(repo, &["rev-list", "--count", "HEAD"]).parse().unwrap()
    }

    /// "…" → Undo Last Commit: HEAD goes back one, the change stays staged, the message is back in the box.
    #[gpui::test]
    fn undo_last_commit_from_the_more_menu(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "git-undo");
        let repo = two_commits(&guard, &wb, cx);
        click(cx, "scm-more");
        assert!(wb.update(cx, |wb, cx| wb.scm.read(cx).menu_open()), "the menu opened");
        click(cx, "m-undo");
        wait_for(cx, "the commit undone", |_| commit_count(&repo) == 1);
        // The one-line box gets the subject; the body goes to the Output.
        wait_for(cx, "the message back", |cx| wb.update(cx, |wb, cx| wb.scm.read(cx).message_text(cx) == "second commit"));
        wait_for(cx, "the body logged", |cx| wb.update(cx, |wb, cx| wb.output.read(cx).state.buffer.text().contains("with a body")));
        assert!(!wb.update(cx, |wb, cx| wb.scm.read(cx).menu_open()), "the menu closed");
        assert_eq!(git(&repo, &["diff", "--cached", "--name-only"]), "A.cs", "the change is staged");
        assert_eq!(std::fs::read_to_string(repo.join("A.cs")).unwrap(), "two\n");
        drop(guard);
    }

    /// Undoing a commit that is already pushed asks first; Cancel leaves it.
    #[gpui::test]
    fn undoing_a_pushed_commit_asks_first(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "git-undo-pushed");
        let repo = two_commits(&guard, &wb, cx);
        let remote = guard.dir.join("remote.git");
        git(&guard.dir, &["init", "-q", "--bare", &remote.to_string_lossy()]);
        git(&repo, &["remote", "add", "origin", &remote.to_string_lossy()]);
        git(&repo, &["push", "-q", "-u", "origin", "HEAD:main"]);
        wb.update(cx, |wb, cx| wb.scm.update(cx, |s, cx| s.refresh(cx)));
        wait_for(cx, "the upstream", |cx| wb.update(cx, |wb, cx| wb.scm.read(cx).status().upstream.is_some()));

        cx.dispatch_action(crate::actions::GitUndoLastCommit);
        wait_for(cx, "the question", |cx| cx.has_pending_prompt());
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        assert_eq!(commit_count(&repo), 2, "Cancel keeps the commit");

        cx.dispatch_action(crate::actions::GitUndoLastCommit);
        wait_for(cx, "the question", |cx| cx.has_pending_prompt());
        cx.simulate_prompt_answer("Undo");
        wait_for(cx, "the commit undone", |_| commit_count(&repo) == 1);
        drop(guard);
    }

    /// Stash Changes… (with a message), Pop Latest Stash, and Drop Stash… (which asks first).
    #[gpui::test]
    fn stash_pop_and_drop(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "git-stash");
        let repo = two_commits(&guard, &wb, cx);
        let file = repo.join("A.cs");
        std::fs::write(&file, "work in progress\n").unwrap();
        wb.update(cx, |wb, cx| wb.scm.update(cx, |s, cx| s.refresh(cx)));
        wait_for(cx, "the change", |cx| wb.update(cx, |wb, cx| !wb.scm.read(cx).status().changes.is_empty()));

        cx.dispatch_action(crate::actions::GitStash);
        cx.run_until_parked();
        assert!(wb.update(cx, |wb, _| matches!(wb.prompt, Some(PromptKind::StashMessage))), "asks for a message");
        wb.update_in(cx, |wb, window, cx| {
            let kind = wb.prompt.take().unwrap();
            wb.close_palette(window, cx);
            wb.complete_prompt(kind, "my wip".into(), window, cx);
        });
        wait_for(cx, "stashed", |_| std::fs::read_to_string(&file).unwrap() == "two\n");
        assert!(git(&repo, &["stash", "list"]).contains("my wip"));

        cx.dispatch_action(crate::actions::GitStashPopLatest);
        wait_for(cx, "popped", |_| std::fs::read_to_string(&file).unwrap() == "work in progress\n");
        assert_eq!(git(&repo, &["stash", "list"]), "", "pop removes the entry");

        // The picker lists the entries; dropping one asks first.
        git(&repo, &["stash", "push", "-q", "-m", "throw away"]);
        cx.dispatch_action(crate::actions::GitStashDrop);
        wait_for(cx, "the stash picker", |cx| wb.update(cx, |wb, _| wb.palette.is_some()));
        wb.update_in(cx, |wb, window, cx| wb.close_palette(window, cx));
        cx.dispatch_action(crate::actions::GitStashEntry { op: "drop".into(), index: 0, message: "On main: throw away".into() });
        wait_for(cx, "the question", |cx| cx.has_pending_prompt());
        cx.simulate_prompt_answer("Drop");
        wait_for(cx, "dropped", |_| git(&repo, &["stash", "list"]).is_empty());
        drop(guard);
    }

    /// A merge that stops at conflicts, then Abort Merge (which asks first) puts everything back.
    #[gpui::test]
    fn merge_with_conflicts_then_abort(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "git-merge");
        let repo = two_commits(&guard, &wb, cx);
        git(&repo, &["switch", "-qc", "side"]);
        std::fs::write(repo.join("A.cs"), "side\n").unwrap();
        git(&repo, &["commit", "-qam", "side change"]);
        git(&repo, &["switch", "-q", "main"]);
        std::fs::write(repo.join("A.cs"), "main\n").unwrap();
        git(&repo, &["commit", "-qam", "main change"]);
        wb.update(cx, |wb, cx| wb.scm.update(cx, |s, cx| s.refresh(cx)));
        cx.run_until_parked();

        cx.dispatch_action(crate::actions::GitMergeWith { branch: "side".into() });
        let merging = |repo: &Path| std::process::Command::new("git").args(["rev-parse", "-q", "--verify", "MERGE_HEAD"]).current_dir(repo).output().unwrap().status.success();
        wait_for(cx, "the merge stopped at conflicts", |_| merging(&repo));
        cx.dispatch_action(crate::actions::GitAbortMerge);
        wait_for(cx, "the question", |cx| cx.has_pending_prompt());
        cx.simulate_prompt_answer("Abort Merge");
        wait_for(cx, "the merge aborted", |_| !merging(&repo));
        assert_eq!(std::fs::read_to_string(repo.join("A.cs")).unwrap(), "main\n");
        drop(guard);
    }

    /// A commit's right-click menu: Create Tag Here… and Revert Commit (which asks first).
    #[gpui::test]
    fn tag_and_revert_from_a_commits_menu(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "git-commit-menu");
        let repo = two_commits(&guard, &wb, cx);
        let first = git(&repo, &["rev-parse", "HEAD~1"]);
        let commit = |id: &str, subject: &str| bf_core::git::Commit {
            id: id.into(),
            parents: Vec::new(),
            author: String::new(),
            email: String::new(),
            when: String::new(),
            date: String::new(),
            refs: String::new(),
            subject: subject.into(),
        };

        wb.update(cx, |wb, cx| wb.scm.update(cx, |s, cx| s.open_commit_menu(commit(&first, "first"), cx)));
        click(cx, "c-tag");
        assert!(wb.update(cx, |wb, _| matches!(&wb.prompt, Some(PromptKind::CreateTag { at: Some(at) }) if *at == first)));
        wb.update_in(cx, |wb, window, cx| {
            let kind = wb.prompt.take().unwrap();
            wb.close_palette(window, cx);
            wb.complete_prompt(kind, "v 1".into(), window, cx);
        });
        wait_for(cx, "the tag", |_| git(&repo, &["tag", "--list"]) == "v-1");
        assert_eq!(git(&repo, &["rev-list", "-n", "1", "v-1"]), first, "the tag is at the picked commit");

        let head = git(&repo, &["rev-parse", "HEAD"]);
        wb.update(cx, |wb, cx| wb.scm.update(cx, |s, cx| s.open_commit_menu(commit(&head, "second commit"), cx)));
        click(cx, "c-revert");
        wait_for(cx, "the question", |cx| cx.has_pending_prompt());
        cx.simulate_prompt_answer("Revert");
        wait_for(cx, "the revert commit", |_| commit_count(&repo) == 3);
        assert!(git(&repo, &["log", "-1", "--format=%s"]).starts_with("Revert \"second commit\""));
        assert_eq!(std::fs::read_to_string(repo.join("A.cs")).unwrap(), "one\n");
        drop(guard);
    }
}
