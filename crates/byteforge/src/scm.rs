//! Source Control view: commit box, staged and unstaged changes, and the git operations behind them.
//! Every git command runs on a background thread; the status is polled so external changes show up.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use bf_core::Workspace;
use bf_core::git::{FileChange, Git, GitStatus};
use gpui::{App, Context, Entity, EventEmitter, FocusHandle, Focusable, PromptLevel, Subscription, Task, Window, div, prelude::*, px, uniform_list};

use crate::assets::icon;
use crate::text_input::{InputEvent, TextInput};
use crate::theme::Theme;
use crate::workbench::tooltip;

pub enum ScmEvent {
    OpenFile(PathBuf),
    OpenDiff(FileChange, bool),
    /// The repository status changed (files, branch or HEAD).
    StatusChanged,
    Log(String),
}

#[derive(Clone)]
enum Entry {
    Header { staged: bool, count: usize },
    File { change: FileChange, staged: bool },
}

pub struct ScmView {
    workspace: Workspace,
    git: Option<Git>,
    status: GitStatus,
    entries: Vec<Entry>,
    message: Entity<TextInput>,
    busy: Option<String>,
    error: Option<String>,
    loaded: bool,
    refresh_task: Option<Task<()>>,
    op_task: Option<Task<()>>,
    _poll: Task<()>,
    _sub: Subscription,
}

impl EventEmitter<ScmEvent> for ScmView {}

impl Focusable for ScmView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.message.focus_handle(cx)
    }
}

/// Theme color key for a change (VS Code's gitDecoration.* names).
pub fn deco_key(change: &FileChange, staged: bool) -> &'static str {
    if change.conflict {
        return "gitDecoration.conflictingResourceForeground";
    }
    if change.untracked {
        return "gitDecoration.untrackedResourceForeground";
    }
    match if staged { change.index } else { change.worktree } {
        'A' => "gitDecoration.addedResourceForeground",
        'D' => "gitDecoration.deletedResourceForeground",
        'R' | 'C' => "gitDecoration.renamedResourceForeground",
        _ if staged => "gitDecoration.stageModifiedResourceForeground",
        _ => "gitDecoration.modifiedResourceForeground",
    }
}

/// Explorer decorations: files get their letter and color, folders the color of a changed descendant.
pub fn decorations(status: &GitStatus, root: &Path) -> HashMap<PathBuf, (char, &'static str)> {
    let mut map = HashMap::new();
    for change in &status.changes {
        let staged = !change.is_unstaged();
        let key = deco_key(change, staged);
        map.insert(change.path.clone(), (change.letter(staged), key));
        let mut dir = change.path.parent();
        while let Some(d) = dir {
            if !d.starts_with(root) || d == root {
                break;
            }
            let entry = map.entry(d.to_path_buf()).or_insert(('•', key));
            if change.conflict {
                entry.1 = key;
            }
            dir = d.parent();
        }
    }
    map
}

impl ScmView {
    pub fn new(workspace: Workspace, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let message = cx.new(|cx| TextInput::new("Message (Enter to commit)", cx));
        let sub = cx.subscribe_in(&message, window, |this, _, event, window, cx| {
            if let InputEvent::Confirmed = event {
                this.commit(window, cx);
            }
        });
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(3)).await;
                if this.update(cx, |v, cx| v.refresh(cx)).is_err() {
                    break;
                }
            }
        });
        Self {
            workspace,
            git: None,
            status: GitStatus::default(),
            entries: Vec::new(),
            message,
            busy: None,
            error: None,
            loaded: false,
            refresh_task: None,
            op_task: None,
            _poll: poll,
            _sub: sub,
        }
    }

    pub fn git(&self) -> Option<&Git> {
        self.git.as_ref()
    }

    pub fn status(&self) -> &GitStatus {
        &self.status
    }

    /// Looks for a repository in the opened folder.
    pub fn open_folder(&mut self, cx: &mut Context<Self>) {
        self.git = None;
        self.loaded = false;
        self.set_status(GitStatus::default(), cx);
        let Some(root) = self.workspace.root() else { return };
        self.refresh_task = Some(cx.spawn(async move |this, cx| {
            let git = cx.background_executor().spawn(async move { Git::discover(&root) }).await;
            let _ = this.update(cx, |v, cx| {
                v.git = git;
                v.loaded = true;
                cx.notify();
                v.refresh(cx);
            });
        }));
    }

    /// Re-reads the status in the background (skipped while an operation runs).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(git) = self.git.clone() else { return };
        if self.busy.is_some() {
            return;
        }
        self.refresh_task = Some(cx.spawn(async move |this, cx| {
            let status = cx.background_executor().spawn(async move { git.status() }).await;
            let _ = this.update(cx, |v, cx| match status {
                Ok(status) => v.set_status(status, cx),
                Err(e) => {
                    v.error = Some(e.to_string());
                    cx.notify();
                }
            });
        }));
    }

    fn set_status(&mut self, status: GitStatus, cx: &mut Context<Self>) {
        if status == self.status && !self.entries.is_empty() {
            return;
        }
        let staged: Vec<&FileChange> = status.changes.iter().filter(|c| c.is_staged()).collect();
        let unstaged: Vec<&FileChange> = status.changes.iter().filter(|c| c.is_unstaged()).collect();
        let mut entries = Vec::with_capacity(status.changes.len() + 2);
        if !staged.is_empty() {
            entries.push(Entry::Header { staged: true, count: staged.len() });
            entries.extend(staged.into_iter().map(|c| Entry::File { change: c.clone(), staged: true }));
        }
        entries.push(Entry::Header { staged: false, count: unstaged.len() });
        entries.extend(unstaged.into_iter().map(|c| Entry::File { change: c.clone(), staged: false }));
        let changed = status != self.status;
        self.entries = entries;
        self.status = status;
        if changed {
            cx.emit(ScmEvent::StatusChanged);
        }
        cx.notify();
    }

    fn run_op(&mut self, label: &'static str, clear_message: bool, op: impl FnOnce(&Git) -> anyhow::Result<String> + Send + 'static, cx: &mut Context<Self>) {
        let Some(git) = self.git.clone() else { return };
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(label.into());
        self.error = None;
        cx.notify();
        self.op_task = Some(cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { op(&git) }).await;
            let _ = this.update(cx, |v, cx| {
                v.busy = None;
                match result {
                    Ok(out) => {
                        if !out.trim().is_empty() {
                            cx.emit(ScmEvent::Log(format!("git {label}: {}", out.trim())));
                        }
                        if clear_message {
                            v.message.update(cx, |m, cx| m.set_text("", cx));
                        }
                    }
                    Err(e) => {
                        cx.emit(ScmEvent::Log(format!("git {label} failed: {e}")));
                        v.error = Some(e.to_string());
                    }
                }
                v.refresh(cx);
                cx.notify();
            });
        }));
    }

    pub fn init(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.root() else { return };
        self.busy = Some("init".into());
        self.op_task = Some(cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { Git::init(&root) }).await;
            let _ = this.update(cx, |v, cx| {
                v.busy = None;
                match result {
                    Ok(git) => {
                        cx.emit(ScmEvent::Log(format!("Initialized a git repository in {}", git.root().display())));
                        v.git = Some(git);
                        v.refresh(cx);
                    }
                    Err(e) => v.error = Some(e.to_string()),
                }
                cx.notify();
            });
        }));
    }

    pub fn stage(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.run_op("add", false, move |g| g.stage(&paths).map(|_| String::new()), cx);
    }

    pub fn unstage(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.run_op("restore --staged", false, move |g| g.unstage(&paths).map(|_| String::new()), cx);
    }

    pub fn stage_all(&mut self, cx: &mut Context<Self>) {
        self.run_op("add", false, |g| g.stage_all().map(|_| String::new()), cx);
    }

    pub fn unstage_all(&mut self, cx: &mut Context<Self>) {
        self.run_op("reset", false, |g| g.unstage_all().map(|_| String::new()), cx);
    }

    /// Discards working tree changes after confirmation (untracked files are deleted).
    pub fn discard(&mut self, changes: Vec<FileChange>, window: &mut Window, cx: &mut Context<Self>) {
        if changes.is_empty() {
            return;
        }
        let untracked = changes.iter().filter(|c| c.untracked).count();
        let what = if changes.len() == 1 {
            let name = changes[0].path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            format!("Discard changes in '{name}'?")
        } else {
            format!("Discard changes in {} files?", changes.len())
        };
        let detail = if untracked > 0 { format!("{untracked} untracked file(s) will be DELETED. This cannot be undone.") } else { "This cannot be undone.".into() };
        let rx = window.prompt(PromptLevel::Warning, &what, Some(&detail), &["Discard", "Cancel"], cx);
        cx.spawn(async move |this, cx| {
            if let Ok(0) = rx.await {
                let _ = this.update(cx, |v, cx| v.run_op("restore", false, move |g| g.discard(&changes).map(|_| String::new()), cx));
            }
        })
        .detach();
    }

    pub fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let message = self.message.read(cx).text().trim().to_owned();
        if message.is_empty() {
            self.error = Some("Enter a commit message first.".into());
            window.focus(&self.message.focus_handle(cx));
            cx.notify();
            return;
        }
        let has_staged = self.status.changes.iter().any(FileChange::is_staged);
        if has_staged {
            self.run_op("commit", true, move |g| g.commit(&message, false), cx);
            return;
        }
        if self.status.changes.is_empty() {
            self.error = Some("There are no changes to commit.".into());
            cx.notify();
            return;
        }
        let rx = window.prompt(PromptLevel::Info, "There are no staged changes.", Some("Stage all changes and commit them?"), &["Stage All & Commit", "Cancel"], cx);
        cx.spawn(async move |this, cx| {
            if let Ok(0) = rx.await {
                let _ = this.update(cx, |v, cx| {
                    v.run_op("commit", true, move |g| {
                        g.stage_all()?;
                        g.commit(&message, false)
                    }, cx)
                });
            }
        })
        .detach();
    }

    pub fn pull(&mut self, cx: &mut Context<Self>) {
        self.run_op("pull", false, |g| g.pull(), cx);
    }

    pub fn push(&mut self, cx: &mut Context<Self>) {
        let status = self.status.clone();
        self.run_op("push", false, move |g| g.push(&status), cx);
    }

    pub fn fetch(&mut self, cx: &mut Context<Self>) {
        self.run_op("fetch", false, |g| g.fetch(), cx);
    }

    /// Pull (fast-forward only), then push.
    pub fn sync(&mut self, cx: &mut Context<Self>) {
        let status = self.status.clone();
        self.run_op("sync", false, move |g| {
            let mut out = if status.upstream.is_some() { g.pull()? } else { String::new() };
            let status = g.status().unwrap_or(status);
            out.push('\n');
            out.push_str(&g.push(&status)?);
            Ok(out)
        }, cx);
    }

    pub fn checkout(&mut self, branch: bf_core::git::Branch, cx: &mut Context<Self>) {
        self.run_op("switch", false, move |g| g.checkout(&branch), cx);
    }

    pub fn create_branch(&mut self, name: String, cx: &mut Context<Self>) {
        self.run_op("switch -c", false, move |g| g.create_branch(&name), cx);
    }
}

impl Render for ScmView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::global(cx);
        let button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px_3()
                .py_1()
                .rounded(t.radius("button.radius", 4.0))
                .bg(t.bg("button.background"))
                .text_color(t.c("button.foreground"))
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .gap_1()
                .hover(|s| s.bg(t.c("button.hoverBackground")))
                .child(label)
        };
        let muted = t.c("foreground.muted");
        let message = |text: String| div().text_size(px(12.)).text_color(muted).child(text);

        if self.workspace.root().is_none() {
            return div().p_4().child(message("Open a folder to use source control.".into())).into_any_element();
        }
        if self.git.is_none() {
            return div()
                .p_4()
                .flex()
                .flex_col()
                .gap_3()
                .child(message(if self.loaded { "The opened folder is not a git repository.".into() } else { "Looking for a git repository…".into() }))
                .when(self.loaded, |d| d.child(button("scm-init", "Initialize Repository").on_click(cx.listener(|v, _, _, cx| v.init(cx)))))
                .when_some(self.error.clone(), |d, e| d.child(div().text_size(px(12.)).text_color(t.c("errorForeground")).child(e)))
                .into_any_element();
        }

        let icon_size = t.m("scm.iconSize", 14.0);
        let tool = |id: &'static str, name: &'static str, tip: &'static str| {
            div()
                .id(id)
                .p(px(3.))
                .rounded(px(4.))
                .cursor_pointer()
                .hover(|s| s.bg(t.c("toolbar.hoverBackground")))
                .tooltip(tooltip(tip))
                .child(icon(name, icon_size, t.c("icon.foreground")))
        };
        let s = &self.status;
        let branch = match (&s.branch, s.detached) {
            (Some(b), _) => b.clone(),
            (None, true) => "(detached HEAD)".into(),
            _ => String::new(),
        };
        let sync = match (s.upstream.is_some(), s.ahead, s.behind) {
            (false, _, _) if s.has_commits => "not published".to_owned(),
            (true, 0, 0) => "up to date".to_owned(),
            (true, a, b) => format!("↑{a} ↓{b}"),
            _ => String::new(),
        };

        let entity = cx.entity();
        let row_h = t.n("list.itemHeight", 22.0);
        let root = self.git.as_ref().map(|g| g.root().to_path_buf()).unwrap_or_default();
        let list = uniform_list("scm-changes", self.entries.len(), move |range, _, cx| {
            let view = entity.read(cx);
            let t = Theme::global(cx);
            range
                .map(|ix| {
                    let entry = view.entries[ix].clone();
                    let entity = entity.clone();
                    let small = |id: &'static str, name: &'static str, tip: &'static str| {
                        div()
                            .id(id)
                            .p(px(2.))
                            .rounded(px(3.))
                            .cursor_pointer()
                            .hover(|s| s.bg(t.c("toolbar.hoverBackground")))
                            .tooltip(tooltip(tip))
                            .child(icon(name, px(14.), t.c("icon.foreground")))
                    };
                    match entry {
                        Entry::Header { staged, count } => {
                            let e1 = entity.clone();
                            let e2 = entity.clone();
                            let row = div()
                                .id(ix)
                                .group("scm-row")
                                .w_full()
                                .h(px(row_h))
                                .px_3()
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_size(t.m("sideBarSectionHeader.fontSize", 11.0))
                                .font_weight(t.weight("sideBarSectionHeader.weight", gpui::FontWeight::BOLD))
                                .text_color(t.c("sideBarSectionHeader.foreground"))
                                .child(div().flex_1().child(if staged { "STAGED CHANGES" } else { "CHANGES" }))
                                .child(
                                    div()
                                        .flex()
                                        .gap_1()
                                        .invisible()
                                        .group_hover("scm-row", |s| s.visible())
                                        .when(!staged && count > 0, |d| {
                                            d.child(small("discard-all", "discard", "Discard All Changes").on_click(move |_, window, cx| {
                                                e1.update(cx, |v, cx| {
                                                    let all = v.status.changes.iter().filter(|c| c.is_unstaged()).cloned().collect();
                                                    v.discard(all, window, cx)
                                                })
                                            }))
                                        })
                                        .child(if staged {
                                            small("unstage-all", "remove", "Unstage All Changes").on_click(move |_, _, cx| e2.update(cx, |v, cx| v.unstage_all(cx)))
                                        } else {
                                            small("stage-all", "add", "Stage All Changes").on_click(move |_, _, cx| e2.update(cx, |v, cx| v.stage_all(cx)))
                                        }),
                                )
                                .child(
                                    div()
                                        .px(px(6.))
                                        .rounded(px(9.))
                                        .text_size(px(11.))
                                        .bg(t.bg("badge.background"))
                                        .text_color(t.c("badge.foreground"))
                                        .child(count.to_string()),
                                );
                            row.into_any_element()
                        }
                        Entry::File { change, staged } => {
                            let name = change.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                            let dir = change.path.parent().and_then(|p| p.strip_prefix(&root).ok()).map(|p| p.display().to_string()).unwrap_or_default();
                            let color = t.c(deco_key(&change, staged));
                            let (e1, e2, e3) = (entity.clone(), entity.clone(), entity.clone());
                            let (c1, c2, c3) = (change.clone(), change.clone(), change.clone());
                            let deleted = (if staged { change.index } else { change.worktree }) == 'D';
                            div()
                                .id(ix)
                                .group("scm-row")
                                .w_full()
                                .h(px(row_h))
                                .pl(px(20.))
                                .pr_3()
                                .flex()
                                .items_center()
                                .gap(t.m("explorer.itemSpacing", 6.0))
                                .cursor_pointer()
                                .text_size(t.m("list.fontSize", 13.0))
                                .hover(|s| s.bg(t.c("list.hoverBackground")))
                                .child(icon("file", t.m("explorer.iconSize", 15.0), t.c(crate::explorer::icon_color_key(&name, false))))
                                .child(div().whitespace_nowrap().text_color(color).when(deleted, |d| d.line_through()).child(name))
                                .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_size(px(11.)).text_color(t.c("foreground.muted")).child(dir))
                                .child(
                                    div()
                                        .flex()
                                        .gap_1()
                                        .invisible()
                                        .group_hover("scm-row", |s| s.visible())
                                        .when(!deleted, |d| {
                                            d.child(small("open", "file", "Open File").on_click(move |_, _, cx| {
                                                cx.stop_propagation();
                                                e1.update(cx, |_, cx| cx.emit(ScmEvent::OpenFile(c1.path.clone())))
                                            }))
                                        })
                                        .when(!staged, |d| {
                                            let e = e2.clone();
                                            let c = c2.clone();
                                            d.child(small("discard", "discard", "Discard Changes").on_click(move |_, window, cx| {
                                                cx.stop_propagation();
                                                e.update(cx, |v, cx| v.discard(vec![c.clone()], window, cx))
                                            }))
                                        })
                                        .child(if staged {
                                            small("unstage", "remove", "Unstage Changes").on_click(move |_, _, cx| {
                                                cx.stop_propagation();
                                                e2.update(cx, |v, cx| v.unstage(vec![c2.path.clone()], cx))
                                            })
                                        } else {
                                            let c = c2.clone();
                                            small("stage", "add", "Stage Changes").on_click(move |_, _, cx| {
                                                cx.stop_propagation();
                                                e2.update(cx, |v, cx| v.stage(vec![c.path.clone()], cx))
                                            })
                                        }),
                                )
                                .child(div().w(px(12.)).flex().justify_center().text_color(color).text_size(px(12.)).child(change.letter(staged).to_string()))
                                .on_click(move |_, _, cx| e3.update(cx, |_, cx| cx.emit(ScmEvent::OpenDiff(c3.clone(), staged))))
                                .into_any_element()
                        }
                    }
                })
                .collect()
        })
        .flex_1();

        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(icon("sourceControl", icon_size, t.c("icon.foreground")))
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(branch).child(div().text_size(px(11.)).text_color(muted).child(sync)))
                    .child(tool("scm-refresh", "refresh", "Refresh").on_click(cx.listener(|v, _, _, cx| v.refresh(cx))))
                    .child(tool("scm-pull", "arrowDown", "Pull").on_click(cx.listener(|v, _, _, cx| v.pull(cx))))
                    .child(tool("scm-push", "arrowUp", "Push").on_click(cx.listener(|v, _, _, cx| v.push(cx))))
                    .child(tool("scm-fetch", "sync", "Fetch").on_click(cx.listener(|v, _, _, cx| v.fetch(cx)))),
            )
            .child(div().px_3().child(self.message.clone()))
            .child(
                div().px_3().child(
                    button("scm-commit", if self.busy.is_some() { "Working…" } else { "✓ Commit" })
                        .w_full()
                        .when(self.busy.is_some(), |d| d.opacity(0.6))
                        .tooltip(tooltip("Commit the staged changes (Enter in the message box)"))
                        .on_click(cx.listener(|v, _, window, cx| v.commit(window, cx))),
                ),
            )
            .when_some(self.busy.clone(), |d, b| d.child(div().px_3().child(message(format!("git {b}…")))))
            .when_some(self.error.clone(), |d, e| d.child(div().px_3().text_size(px(12.)).text_color(t.c("errorForeground")).child(e)))
            .child(list)
            .into_any_element()
    }
}
