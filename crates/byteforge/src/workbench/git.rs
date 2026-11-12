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

    /// Reloads unmodified editors whose file changed on disk.
    pub(super) fn reload_changed_tabs(&self, cx: &mut Context<Self>) {
        for tab in &self.tabs {
            let Some(path) = &tab.path else { continue };
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
                let existing = wb.tabs.iter().position(|t| t.title == title && matches!(t.page, Some(Page::Diff(_))));
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
                let diff = cx.new(|cx| crate::diff_view::DiffView::new(editor.clone(), language, cx));
                let sub = cx.subscribe_in(&diff, window, |wb: &mut Self, diff, event: &crate::diff_view::DiffEvent, window, cx| match event {
                    crate::diff_view::DiffEvent::OpenFile(path, at) => wb.open_file(path.clone(), Some(*at), window, cx),
                    crate::diff_view::DiffEvent::Change(action, path, change) => wb.diff_change(diff.clone(), *action, path.clone(), *change, window, cx),
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
        let apply = move |wb: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
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
            let _ = wb;
        };
        if action != ChangeAction::Revert {
            return apply(self, window, cx);
        }
        let rx = window.prompt(PromptLevel::Warning, "Revert this change?", Some("The lines go back to what git has; what you changed in them is lost."), &["Revert", "Cancel"], cx);
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(0) = rx.await {
                let _ = this.update_in(cx, |wb, window, cx| apply(wb, window, cx));
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
        for (k, v) in [("user.name", "T"), ("user.email", "t@example.com"), ("commit.gpgsign", "false")] {
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

    /// Local changes in the way: the switch fails with a question instead of silently, and "Stash Changes
    /// and Switch" puts them on the stash and switches.
    #[gpui::test]
    fn a_blocked_switch_asks_and_can_stash(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "blocked-switch");
        let repo = guard.dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        for (k, v) in [("user.name", "T"), ("user.email", "t@example.com"), ("commit.gpgsign", "false")] {
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
        for (k, v) in [("user.name", "T"), ("user.email", "t@example.com"), ("commit.gpgsign", "false")] {
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
}
