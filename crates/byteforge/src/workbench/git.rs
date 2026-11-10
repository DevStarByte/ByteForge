//! Source control events, git gutter bases and diff tabs.

use super::*;

impl Workbench {
    pub(super) fn on_scm_event(&mut self, scm: &Entity<ScmView>, event: &ScmEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            ScmEvent::OpenFile(path) => self.open_file(path.clone(), None, window, cx),
            ScmEvent::OpenDiff(change, staged) => self.open_diff(change.clone(), *staged, window, cx),
            ScmEvent::OpenCommitDiff(commit, file) => self.open_commit_diff(commit.clone(), file.clone(), window, cx),
            ScmEvent::Log(text) => self.log(text),
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

    /// Opens a read-only unified diff of a change in a tab (reused when it is already open).
    pub(super) fn open_diff(&mut self, change: bf_core::git::FileChange, staged: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(git) = self.scm.read(cx).git().cloned() else { return };
        let name = change.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let title = format!("{name} ({})", if staged { "Staged" } else if change.untracked { "Untracked" } else { "Working Tree" });
        let rel = change.path.strip_prefix(git.root()).unwrap_or(&change.path).display().to_string().replace('\\', "/");
        let task = cx.background_spawn(async move {
            let (old, new) = git.diff_texts(&change, staged);
            let (a, b) = if staged { (format!("HEAD:{rel}"), format!("index:{rel}")) } else { (format!("index:{rel}"), rel.clone()) };
            bf_core::git::unified_diff(&old, &new, &a, &b)
        });
        self.show_diff_tab(title, task, window, cx);
    }

    /// Opens what a commit changed in a file (first parent ↔ commit) as a read-only diff tab.
    pub(super) fn open_commit_diff(&mut self, commit: bf_core::git::Commit, file: bf_core::git::CommitFile, window: &mut Window, cx: &mut Context<Self>) {
        let Some(git) = self.scm.read(cx).git().cloned() else { return };
        let name = file.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let title = format!("{name} ({})", commit.short());
        let task = cx.background_spawn(async move {
            let (old, new) = git.commit_diff_texts(&commit, &file);
            let rel = git.relative(&file.path);
            let orig = file.orig.as_deref().map_or_else(|| rel.clone(), |o| git.relative(o));
            let a = match commit.parents.first() {
                Some(p) if file.status != 'A' => format!("{}:{orig}", &p[..p.len().min(7)]),
                _ => "/dev/null".to_owned(),
            };
            let b = if file.status == 'D' { "/dev/null".to_owned() } else { format!("{}:{rel}", commit.short()) };
            bf_core::git::unified_diff(&old, &new, &a, &b)
        });
        self.show_diff_tab(title, task, window, cx);
    }

    /// Shows a unified diff in a read-only tab titled `title`, reusing an open tab of the same title.
    pub(super) fn show_diff_tab(&mut self, title: String, task: Task<String>, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let diff = task.await;
            let _ = this.update_in(cx, |wb, window, cx| {
                let text = if diff.is_empty() { "No textual changes (binary file or only mode changes).".to_owned() } else { diff };
                let backgrounds: Vec<(usize, &'static str)> = text
                    .lines()
                    .enumerate()
                    .filter_map(|(i, line)| {
                        let key = if line.starts_with("+++") || line.starts_with("---") {
                            return None;
                        } else if line.starts_with('+') {
                            "diffEditor.insertedLineBackground"
                        } else if line.starts_with('-') {
                            "diffEditor.removedLineBackground"
                        } else if line.starts_with("@@") {
                            "diffEditor.hunkHeaderBackground"
                        } else {
                            return None;
                        };
                        Some((i, key))
                    })
                    .collect();
                if let Some(ix) = wb.tabs.iter().position(|t| t.path.is_none() && t.title == title && t.editor.read(cx).read_only) {
                    wb.tabs[ix].editor.update(cx, |e, cx| {
                        e.reload_text(&text, cx);
                        e.line_backgrounds = backgrounds;
                    });
                    wb.activate(ix, window, cx);
                    return;
                }
                let editor = cx.new(|cx| {
                    let mut e = EditorView::new(&text, None, cx);
                    e.read_only = true;
                    e.show_indent_guides = false;
                    e.line_backgrounds = backgrounds;
                    e
                });
                wb.add_tab(editor, None, title, false, window, cx);
            });
        })
        .detach();
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
