//! Source Control view: commit box, staged and unstaged changes, and the git operations behind them.
//! Every git command runs on a background thread; the status is polled so external changes show up.
//! Below the changes, the History section lists the commits of the current branch; a commit expands to
//! the files it changed, and a file opens the commit's diff.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use bf_core::Workspace;
use bf_core::git::{Commit, CommitFile, FileChange, Git, GitStatus};
use gpui::{App, Context, Entity, EventEmitter, FocusHandle, Focusable, MouseButton, MouseDownEvent, Pixels, Point, PromptLevel, Subscription, Task, Window, div, prelude::*, px, uniform_list};

use crate::assets::icon;
use crate::text_input::{InputEvent, TextInput};
use crate::theme::Theme;
use crate::workbench::tooltip;

pub enum ScmEvent {
    OpenFile(PathBuf),
    OpenDiff(FileChange, bool),
    /// A file of a commit in the History section: show what the commit changed in it.
    OpenCommitDiff(Commit, CommitFile),
    /// The repository status changed (files, branch or HEAD).
    StatusChanged,
    Log(String),
    /// Switching to the branch failed (git's message): the workbench shows it, and offers to stash local
    /// changes that are in the way.
    CheckoutFailed(bf_core::git::Branch, String),
}

#[derive(Clone)]
enum Entry {
    Header { staged: bool, count: usize },
    File { change: FileChange, staged: bool },
    HistoryHeader,
    Commit { commit: Commit, expanded: bool },
    CommitFile { commit: Commit, file: CommitFile },
    /// Placeholder under an expanded commit while its files load.
    CommitLoading,
    LoadMore,
}

/// Commits loaded per page of the History section.
const HISTORY_PAGE: usize = 100;

#[derive(Default)]
struct History {
    open: bool,
    commits: Vec<Commit>,
    /// HEAD the list was loaded for (reloaded when HEAD moves).
    head: Option<String>,
    more: bool,
    loading: bool,
    expanded: HashSet<String>,
    files: HashMap<String, Vec<CommitFile>>,
    task: Option<Task<()>>,
    files_tasks: HashMap<String, Task<()>>,
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
    history: History,
    refresh_task: Option<Task<()>>,
    op_task: Option<Task<()>>,
    /// The "…" menu, open at this position.
    more_menu: Option<Point<Pixels>>,
    /// A commit's right-click menu in HISTORY.
    commit_menu: Option<(Commit, Point<Pixels>)>,
    _poll: Task<()>,
    _sub: Subscription,
}

impl EventEmitter<ScmEvent> for ScmView {}

/// What to do with a successful operation's output.
type OpDone = Box<dyn FnOnce(&mut ScmView, String, &mut Context<ScmView>)>;

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

/// Theme color key for a file changed by a commit.
fn commit_file_key(status: char) -> &'static str {
    match status {
        'A' => "gitDecoration.addedResourceForeground",
        'D' => "gitDecoration.deletedResourceForeground",
        'R' | 'C' => "gitDecoration.renamedResourceForeground",
        _ => "gitDecoration.modifiedResourceForeground",
    }
}

/// Compact age for the history list: "14 minutes ago" → "14m", "3 weeks ago" → "3w".
fn short_age(when: &str) -> String {
    let mut words = when.split_whitespace();
    let (Some(n), Some(unit)) = (words.next(), words.next()) else { return when.to_owned() };
    let unit = unit.trim_end_matches(',');
    let suffix = match unit.trim_end_matches('s') {
        "second" => "s",
        "minute" => "m",
        "hour" => "h",
        "day" => "d",
        "week" => "w",
        "month" => "mo",
        "year" => "y",
        _ => return when.to_owned(),
    };
    format!("{n}{suffix}")
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

/// The keys that commit from the message box.
const COMMIT_KEY: &str = if cfg!(target_os = "macos") { "⌘Enter" } else { "Ctrl+Enter" };

impl ScmView {
    pub fn new(workspace: Workspace, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Like VS Code's: Enter starts a new line (subject, blank line, body), Ctrl+Enter commits.
        let message = cx.new(|cx| TextInput::multiline(format!("Message ({COMMIT_KEY} to commit)"), 10, cx));
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
            history: History { open: true, ..Default::default() },
            refresh_task: None,
            op_task: None,
            more_menu: None,
            commit_menu: None,
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
        self.history = History { open: self.history.open, ..Default::default() };
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
        let changed = status != self.status;
        self.status = status;
        if self.git.is_some() && self.history.head != self.status.head {
            self.load_history(false, cx);
        }
        self.rebuild_entries();
        if changed {
            cx.emit(ScmEvent::StatusChanged);
        }
        cx.notify();
    }

    fn rebuild_entries(&mut self) {
        let status = &self.status;
        let staged: Vec<&FileChange> = status.changes.iter().filter(|c| c.is_staged()).collect();
        let unstaged: Vec<&FileChange> = status.changes.iter().filter(|c| c.is_unstaged()).collect();
        let mut entries = Vec::with_capacity(status.changes.len() + 3 + self.history.commits.len());
        if !staged.is_empty() {
            entries.push(Entry::Header { staged: true, count: staged.len() });
            entries.extend(staged.into_iter().map(|c| Entry::File { change: c.clone(), staged: true }));
        }
        entries.push(Entry::Header { staged: false, count: unstaged.len() });
        entries.extend(unstaged.into_iter().map(|c| Entry::File { change: c.clone(), staged: false }));
        if status.has_commits {
            let h = &self.history;
            entries.push(Entry::HistoryHeader);
            if h.open {
                for commit in &h.commits {
                    let expanded = h.expanded.contains(&commit.id);
                    entries.push(Entry::Commit { commit: commit.clone(), expanded });
                    if expanded {
                        match h.files.get(&commit.id) {
                            Some(files) => entries.extend(files.iter().map(|f| Entry::CommitFile { commit: commit.clone(), file: f.clone() })),
                            None => entries.push(Entry::CommitLoading),
                        }
                    }
                }
                if h.more {
                    entries.push(Entry::LoadMore);
                }
            }
        }
        self.entries = entries;
    }

    /// Loads the first page of the history (or, with `more`, the next page).
    fn load_history(&mut self, more: bool, cx: &mut Context<Self>) {
        let Some(git) = self.git.clone() else { return };
        let head = self.status.head.clone();
        let skip = if more { self.history.commits.len() } else { 0 };
        if !more {
            self.history.head = head.clone();
        }
        self.history.loading = true;
        self.history.task = Some(cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { git.log(HISTORY_PAGE, skip) }).await;
            let _ = this.update(cx, |v, cx| {
                v.history.loading = false;
                if v.history.head != head {
                    return; // HEAD moved meanwhile; a newer load is on its way.
                }
                match result {
                    Ok(commits) => {
                        v.history.more = commits.len() == HISTORY_PAGE;
                        if more {
                            v.history.commits.extend(commits);
                        } else {
                            let ids: HashSet<&str> = commits.iter().map(|c| c.id.as_str()).collect();
                            v.history.expanded.retain(|id| ids.contains(id.as_str()));
                            v.history.files.retain(|id, _| ids.contains(id.as_str()));
                            v.history.commits = commits;
                        }
                    }
                    Err(e) => cx.emit(ScmEvent::Log(format!("git log failed: {e}"))),
                }
                v.rebuild_entries();
                cx.notify();
            });
        }));
    }

    fn toggle_history(&mut self, cx: &mut Context<Self>) {
        self.history.open = !self.history.open;
        self.rebuild_entries();
        cx.notify();
    }

    /// Expands or collapses a commit; its changed files load on first expansion.
    fn toggle_commit(&mut self, commit: Commit, cx: &mut Context<Self>) {
        if !self.history.expanded.remove(&commit.id) {
            self.history.expanded.insert(commit.id.clone());
            if !self.history.files.contains_key(&commit.id)
                && let Some(git) = self.git.clone()
            {
                let id = commit.id.clone();
                let task = cx.spawn(async move |this, cx| {
                    let id = commit.id.clone();
                    let result = cx.background_executor().spawn(async move { git.commit_files(&commit) }).await;
                    let _ = this.update(cx, |v, cx| {
                        v.history.files_tasks.remove(&id);
                        match result {
                            Ok(files) => {
                                v.history.files.insert(id, files);
                            }
                            Err(e) => {
                                v.history.files.insert(id, Vec::new());
                                cx.emit(ScmEvent::Log(format!("git diff-tree failed: {e}")));
                            }
                        }
                        v.rebuild_entries();
                        cx.notify();
                    });
                });
                self.history.files_tasks.insert(id, task);
            }
        }
        self.rebuild_entries();
        cx.notify();
    }

    fn run_op(&mut self, label: &'static str, clear_message: bool, op: impl FnOnce(&Git) -> anyhow::Result<String> + Send + 'static, cx: &mut Context<Self>) {
        self.run_op_with(label, clear_message, op, None, cx);
    }

    /// [`Self::run_op`]; on success `done` gets the operation's output (which isn't logged then).
    fn run_op_with(&mut self, label: &'static str, clear_message: bool, op: impl FnOnce(&Git) -> anyhow::Result<String> + Send + 'static, done: Option<OpDone>, cx: &mut Context<Self>) {
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
                        if clear_message {
                            v.message.update(cx, |m, cx| m.set_text("", cx));
                        }
                        if let Some(done) = done {
                            done(v, out, cx);
                        } else if !out.trim().is_empty() {
                            cx.emit(ScmEvent::Log(format!("git {label}: {}", out.trim())));
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

    /// Switches to `branch`; with `stash`, local changes go on the stash first. A failure is reported as
    /// [`ScmEvent::CheckoutFailed`] (not only to the log, where it's easy to miss).
    pub fn checkout(&mut self, branch: bf_core::git::Branch, stash: bool, cx: &mut Context<Self>) {
        let Some(git) = self.git.clone() else { return };
        if self.busy.is_some() {
            cx.emit(ScmEvent::Log(format!("git is busy ({}); try switching again in a moment.", self.busy.clone().unwrap_or_default())));
            return;
        }
        self.busy = Some("switch".into());
        self.error = None;
        cx.notify();
        let target = branch.clone();
        self.op_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    if stash {
                        git.stash_push(&format!("ByteForge: before switching to {}", target.name))?;
                    }
                    git.checkout(&target)
                })
                .await;
            let _ = this.update(cx, |v, cx| {
                v.busy = None;
                match result {
                    Ok(out) => cx.emit(ScmEvent::Log(format!("Switched to {}{}{}", branch.name, if stash { " (local changes are on the stash)" } else { "" }, if out.trim().is_empty() { String::new() } else { format!(": {}", out.trim()) }))),
                    Err(e) => {
                        v.error = Some(e.to_string());
                        cx.emit(ScmEvent::CheckoutFailed(branch, e.to_string()));
                    }
                }
                v.refresh(cx);
                cx.notify();
            });
        }));
    }

    pub fn create_branch(&mut self, name: String, cx: &mut Context<Self>) {
        self.run_op("switch -c", false, move |g| g.create_branch(&name), cx);
    }

    /// Whether HEAD is already on the remote branch (undoing or amending it rewrites published history).
    fn head_is_pushed(&self) -> bool {
        self.status.upstream.is_some() && self.status.ahead == 0 && self.status.has_commits
    }

    /// Asks first when `pushed` (the commit is on the remote already), then runs `then`.
    fn confirm_rewrite(&mut self, pushed: bool, what: &str, then: impl FnOnce(&mut Self, &mut Context<Self>) + 'static, window: &mut Window, cx: &mut Context<Self>) {
        if !pushed {
            return then(self, cx);
        }
        let detail = "It is already pushed: the remote branch keeps it, so pushing afterwards needs a force push, and others who pulled it are affected.";
        let rx = window.prompt(PromptLevel::Warning, &format!("{what} a pushed commit?"), Some(detail), &[what, "Cancel"], cx);
        cx.spawn(async move |this, cx| {
            if let Ok(0) = rx.await {
                let _ = this.update(cx, then);
            }
        })
        .detach();
    }

    /// "Undo Last Commit": HEAD goes back one commit; its changes stay staged and its message goes back
    /// into the message box, so it can be fixed and committed again.
    pub fn undo_last_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.status.has_commits {
            cx.emit(ScmEvent::Log("There is no commit to undo.".into()));
            return;
        }
        let pushed = self.head_is_pushed();
        self.confirm_rewrite(
            pushed,
            "Undo",
            |v, cx| {
                v.run_op_with(
                    "reset --soft",
                    false,
                    |g| g.undo_last_commit(),
                    Some(Box::new(|v, message, cx| {
                        let subject = message.lines().next().unwrap_or_default().to_owned();
                        cx.emit(ScmEvent::Log(format!("Undid the commit \"{subject}\": its changes are staged again.")));
                        // The whole message (subject and body) goes back into the box.
                        v.message.update(cx, |m, cx| m.set_text(message.trim_end(), cx));
                    })),
                    cx,
                )
            },
            window,
            cx,
        );
    }

    /// "Commit (Amend)": the staged changes go into the last commit; the message box's text replaces its
    /// message (left empty, the message stays).
    pub fn commit_amend(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.status.has_commits {
            self.error = Some("There is no commit to amend yet.".into());
            cx.notify();
            return;
        }
        let message = self.message.read(cx).text().trim().to_owned();
        if message.is_empty() && !self.status.changes.iter().any(FileChange::is_staged) {
            self.error = Some("Stage changes, or type a new message, to amend the last commit.".into());
            cx.notify();
            return;
        }
        let pushed = self.head_is_pushed();
        self.confirm_rewrite(pushed, "Amend", move |v, cx| v.run_op("commit --amend", true, move |g| g.amend(&message), cx), window, cx);
    }

    /// Puts the changes (untracked files too) on the stash; `message` names the entry.
    pub fn stash(&mut self, message: String, cx: &mut Context<Self>) {
        if self.status.changes.is_empty() {
            cx.emit(ScmEvent::Log("There are no changes to stash.".into()));
            return;
        }
        let message = if message.trim().is_empty() { "ByteForge stash".to_owned() } else { message };
        self.run_op("stash", false, move |g| g.stash_push(&message), cx);
    }

    /// Pops, applies or drops stash entry `index`.
    pub fn stash_entry(&mut self, op: bf_core::git::StashOp, index: usize, cx: &mut Context<Self>) {
        let label = match op {
            bf_core::git::StashOp::Pop => "stash pop",
            bf_core::git::StashOp::Apply => "stash apply",
            bf_core::git::StashOp::Drop => "stash drop",
        };
        self.run_op(label, false, move |g| g.stash_op(op, index), cx);
    }

    /// Discards every unstaged change (asks first; untracked files are deleted).
    pub fn discard_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let changes: Vec<FileChange> = self.status.changes.iter().filter(|c| c.is_unstaged()).cloned().collect();
        if changes.is_empty() {
            cx.emit(ScmEvent::Log("There are no unstaged changes to discard.".into()));
            return;
        }
        self.discard(changes, window, cx);
    }

    /// Merges `branch` into the current branch. Stopping at conflicts is reported with what to do next.
    pub fn merge(&mut self, branch: String, cx: &mut Context<Self>) {
        self.run_op(
            "merge",
            false,
            move |g| {
                g.merge(&branch).map_err(|e| {
                    if g.is_merging() {
                        anyhow::anyhow!("{branch} has conflicting changes. Resolve the conflicts (Merge Editor), stage the files and commit, or Git: Abort Merge.\n{e}")
                    } else {
                        e
                    }
                })
            },
            cx,
        );
    }

    /// Stops a merge that stopped at conflicts, back to before it (asks first).
    pub fn abort_merge(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(git) = self.git.clone() else { return };
        if !git.is_merging() {
            cx.emit(ScmEvent::Log("No merge is in progress.".into()));
            return;
        }
        let rx = window.prompt(PromptLevel::Warning, "Abort the merge?", Some("The files go back to how they were before the merge; conflict resolutions made so far are lost."), &["Abort Merge", "Cancel"], cx);
        cx.spawn(async move |this, cx| {
            if let Ok(0) = rx.await {
                let _ = this.update(cx, |v, cx| v.run_op("merge --abort", false, |g| g.abort_merge(), cx));
            }
        })
        .detach();
    }

    pub fn rename_branch(&mut self, name: String, cx: &mut Context<Self>) {
        self.run_op("branch -m", false, move |g| g.rename_branch(&name), cx);
    }

    /// A tag at `at` (a commit id) or at HEAD.
    pub fn create_tag(&mut self, name: String, at: Option<String>, cx: &mut Context<Self>) {
        self.run_op_with(
            "tag",
            false,
            move |g| g.create_tag(&name, at.as_deref()).map(|_| name),
            Some(Box::new(|v, name, cx| {
                cx.emit(ScmEvent::Log(format!("Created the tag {name}. \"Git: Push Tags\" publishes it.")));
                v.load_history(false, cx);
            })),
            cx,
        );
    }

    pub fn push_tags(&mut self, cx: &mut Context<Self>) {
        self.run_op("push --tags", false, |g| g.push_tags(), cx);
    }

    /// A new commit that undoes `id`.
    pub fn revert_commit(&mut self, id: String, cx: &mut Context<Self>) {
        self.run_op("revert", false, move |g| g.revert_commit(&id), cx);
    }

    #[cfg(test)]
    #[cfg(test)]
    pub fn message_input(&self) -> Entity<TextInput> {
        self.message.clone()
    }

    #[cfg(test)]
    pub fn message_text(&self, cx: &App) -> String {
        self.message.read(cx).text().to_owned()
    }

    #[cfg(test)]
    pub fn menu_open(&self) -> bool {
        self.more_menu.is_some() || self.commit_menu.is_some()
    }

    /// Opens a commit's right-click menu (what a right-click on its HISTORY row does).
    #[cfg(test)]
    pub fn open_commit_menu(&mut self, commit: Commit, cx: &mut Context<Self>) {
        self.commit_menu = Some((commit, gpui::point(px(40.), px(200.))));
        cx.notify();
    }

    /// The current branch (for Rename Branch's prompt).
    pub fn branch(&self) -> Option<&str> {
        self.status.branch.as_deref()
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
                                .relative()
                                .pl(px(20.))
                                .pr_3()
                                .flex()
                                .items_center()
                                .gap(t.m("explorer.itemSpacing", 6.0))
                                .cursor_pointer()
                                .text_size(t.m("list.fontSize", 13.0))
                                .hover(|s| s.bg(t.c("list.hoverBackground")))
                                .child({
                                    let (icon_name, color_key) = crate::explorer::file_icon(&name, false, false);
                                    div().flex_none().flex().child(icon(&icon_name, t.m("explorer.iconSize", 15.0), crate::explorer::file_icon_color(&t, &color_key)))
                                })
                                .child(div().min_w(px(24.)).overflow_hidden().whitespace_nowrap().text_ellipsis().text_color(color).when(deleted, |d| d.line_through()).child(name))
                                .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_size(px(11.)).text_color(t.c("foreground.muted")).child(dir))
                                .child(
                                    hover_actions(&t)
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
                                .child(div().flex_none().w(px(12.)).flex().justify_center().text_color(color).text_size(px(12.)).child(change.letter(staged).to_string()))
                                .on_click(move |_, _, cx| e3.update(cx, |_, cx| cx.emit(ScmEvent::OpenDiff(c3.clone(), staged))))
                                .into_any_element()
                        }
                        Entry::HistoryHeader => {
                            let (e1, e2) = (entity.clone(), entity.clone());
                            div()
                                .id(ix)
                                .group("scm-row")
                                .w_full()
                                .h(px(row_h))
                                .mt_2()
                                .px_3()
                                .flex()
                                .items_center()
                                .gap_1()
                                .cursor_pointer()
                                .text_size(t.m("sideBarSectionHeader.fontSize", 11.0))
                                .font_weight(t.weight("sideBarSectionHeader.weight", gpui::FontWeight::BOLD))
                                .text_color(t.c("sideBarSectionHeader.foreground"))
                                .child(icon(if view.history.open { "chevronDown" } else { "chevronRight" }, px(14.), t.c("icon.foreground")))
                                .child(div().flex_1().child("HISTORY"))
                                .when(view.history.loading, |d| d.child(div().text_size(px(11.)).text_color(t.c("foreground.muted")).font_weight(gpui::FontWeight::NORMAL).child("loading…")))
                                .child(
                                    div().invisible().group_hover("scm-row", |s| s.visible()).child(small("history-refresh", "refresh", "Reload History").on_click(move |_, _, cx| {
                                        cx.stop_propagation();
                                        e1.update(cx, |v, cx| v.load_history(false, cx))
                                    })),
                                )
                                .children(remote_actions(view, &t, &entity))
                                .on_click(move |_, _, cx| e2.update(cx, |v, cx| v.toggle_history(cx)))
                                .into_any_element()
                        }
                        Entry::Commit { commit, expanded } => {
                            let muted = t.c("foreground.muted");
                            let refs = commit.refs.replace("HEAD -> ", "");
                            let merge = commit.parents.len() > 1;
                            let mut tip = format!("{}\n\n{} <{}>\n{} ({})\n{}", commit.subject, commit.author, commit.email, commit.date, commit.when, commit.id);
                            if !commit.refs.is_empty() {
                                tip.push_str(&format!("\n{}", commit.refs));
                            }
                            let e = entity.clone();
                            let c = commit.clone();
                            div()
                                .id(ix)
                                .w_full()
                                .h(px(row_h))
                                .px_3()
                                .flex()
                                .items_center()
                                .gap_1()
                                .cursor_pointer()
                                .text_size(t.m("list.fontSize", 13.0))
                                .hover(|s| s.bg(t.c("list.hoverBackground")))
                                .overflow_hidden()
                                .tooltip(tooltip(tip))
                                .child(icon(if expanded { "chevronDown" } else { "chevronRight" }, px(14.), t.c("icon.foreground")))
                                .child(icon("gitCommit", px(14.), if merge { muted } else { t.c("icon.foreground") }))
                                .child(div().flex_1().min_w(px(60.)).overflow_hidden().whitespace_nowrap().text_ellipsis().child(commit.subject.clone()))
                                .when(!refs.is_empty(), |d| {
                                    d.child(
                                        div()
                                            .flex_shrink()
                                            .min_w(px(24.))
                                            .max_w(px(110.))
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .px(px(5.))
                                            .rounded(px(9.))
                                            .text_size(px(10.))
                                            .bg(t.bg("badge.background"))
                                            .text_color(t.c("badge.foreground"))
                                            .child(refs),
                                    )
                                })
                                .child(div().flex_none().whitespace_nowrap().text_size(px(11.)).text_color(muted).child(short_age(&commit.when)))
                                .on_mouse_down(MouseButton::Right, {
                                    let (e, c) = (e.clone(), c.clone());
                                    move |ev: &MouseDownEvent, _, cx| {
                                        cx.stop_propagation();
                                        e.update(cx, |v, cx| {
                                            v.more_menu = None;
                                            v.commit_menu = Some((c.clone(), ev.position));
                                            cx.notify();
                                        })
                                    }
                                })
                                .on_click(move |_, _, cx| e.update(cx, |v, cx| v.toggle_commit(c.clone(), cx)))
                                .into_any_element()
                        }
                        Entry::CommitFile { commit, file } => {
                            let name = file.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                            let dir = file.path.parent().and_then(|p| p.strip_prefix(&root).ok()).map(|p| p.display().to_string()).unwrap_or_default();
                            let color = t.c(commit_file_key(file.status));
                            let deleted = file.status == 'D';
                            let (e1, e2) = (entity.clone(), entity.clone());
                            let path = file.path.clone();
                            div()
                                .id(ix)
                                .group("scm-row")
                                .w_full()
                                .h(px(row_h))
                                .relative()
                                .pl(px(40.))
                                .pr_3()
                                .flex()
                                .items_center()
                                .gap(t.m("explorer.itemSpacing", 6.0))
                                .cursor_pointer()
                                .text_size(t.m("list.fontSize", 13.0))
                                .hover(|s| s.bg(t.c("list.hoverBackground")))
                                .when_some(file.orig.as_ref(), |d, orig| d.tooltip(tooltip(format!("Renamed from {}", orig.strip_prefix(&root).unwrap_or(orig).display()))))
                                .child({
                                    let (icon_name, color_key) = crate::explorer::file_icon(&name, false, false);
                                    div().flex_none().flex().child(icon(&icon_name, t.m("explorer.iconSize", 15.0), crate::explorer::file_icon_color(&t, &color_key)))
                                })
                                .child(div().min_w(px(24.)).overflow_hidden().whitespace_nowrap().text_ellipsis().text_color(color).when(deleted, |d| d.line_through()).child(name))
                                .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_size(px(11.)).text_color(t.c("foreground.muted")).child(dir))
                                .when(!deleted, |d| {
                                    d.child(hover_actions(&t).child(small("open", "file", "Open File").on_click(move |_, _, cx| {
                                        cx.stop_propagation();
                                        e1.update(cx, |_, cx| cx.emit(ScmEvent::OpenFile(path.clone())))
                                    })))
                                })
                                .child(div().flex_none().w(px(12.)).flex().justify_center().text_color(color).text_size(px(12.)).child(file.status.to_string()))
                                .on_click(move |_, _, cx| e2.update(cx, |_, cx| cx.emit(ScmEvent::OpenCommitDiff(commit.clone(), file.clone()))))
                                .into_any_element()
                        }
                        Entry::CommitLoading => div()
                            .id(ix)
                            .h(px(row_h))
                            .pl(px(40.))
                            .flex()
                            .items_center()
                            .text_size(px(12.))
                            .text_color(t.c("foreground.muted"))
                            .child("Loading…")
                            .into_any_element(),
                        Entry::LoadMore => {
                            let e = entity.clone();
                            div()
                                .id(ix)
                                .h(px(row_h))
                                .px_3()
                                .pl(px(40.))
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .text_size(px(12.))
                                .text_color(t.c_or("textLink.foreground", 0x3794ffff))
                                .hover(|s| s.underline())
                                .child(if view.history.loading { "Loading…" } else { "Load More Commits" })
                                .on_click(move |_, _, cx| {
                                    e.update(cx, |v, cx| {
                                        if !v.history.loading {
                                            v.load_history(true, cx)
                                        }
                                    })
                                })
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
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_baseline()
                            .gap(px(6.))
                            .whitespace_nowrap()
                            // The branch name gives way last: the sync note shortens first.
                            .child(div().flex_shrink_0().max_w_full().overflow_hidden().text_ellipsis().child(branch))
                            .child(div().min_w_0().overflow_hidden().text_ellipsis().text_size(px(11.)).text_color(muted).child(sync)),
                    )
                    .child(tool("scm-refresh", "refresh", "Refresh").on_click(cx.listener(|v, _, _, cx| v.refresh(cx))))
                    .child(tool("scm-more", "more", "More Actions… (undo commit, stash, merge, tags)").debug_selector(|| "scm-more".into()).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|v, ev: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            v.commit_menu = None;
                            v.more_menu = if v.more_menu.is_some() { None } else { Some(ev.position) };
                            cx.notify();
                        }),
                    )),
            )
            .child(div().px_3().flex().flex_col().debug_selector(|| "scm-message".into()).child(self.message.clone()))
            .child(
                div().px_3().flex().flex_col().child(
                    button("scm-commit", if self.busy.is_some() { "Working…" } else { "✓ Commit" })
                        .debug_selector(|| "scm-commit".into())
                        .w_full()
                        .when(self.busy.is_some(), |d| d.opacity(0.6))
                        .tooltip(tooltip(format!("Commit the staged changes ({COMMIT_KEY} in the message box)")))
                        .on_click(cx.listener(|v, _, window, cx| v.commit(window, cx))),
                ),
            )
            .when_some(self.busy.clone(), |d, b| d.child(div().px_3().child(message(format!("git {b}…")))))
            .when_some(self.error.clone(), |d, e| d.child(div().px_3().text_size(px(12.)).text_color(t.c("errorForeground")).child(e)))
            .child(list)
            .children(self.render_menus(cx))
            .into_any_element()
    }
}

impl ScmView {
    /// The "…" menu and a commit's right-click menu. Entries run the same actions as the command palette.
    fn render_menus(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        use crate::actions::*;
        let t = Theme::global(cx);
        let muted = t.c("foreground.muted");
        let entry = |id: &'static str, label: &'static str, action: Box<dyn gpui::Action>, cx: &mut Context<Self>| {
            crate::menu::item(&t, id, label, None).debug_selector(move || id.into()).on_click(cx.listener(move |v, _, window, cx| {
                v.more_menu = None;
                v.commit_menu = None;
                cx.notify();
                window.dispatch_action(action.boxed_clone(), cx);
            }))
        };
        let section = |label: &'static str| div().px_3().pt_1().text_size(px(11.)).text_color(muted).child(label);
        let close = cx.listener(|v, _, _, cx| {
            v.more_menu = None;
            v.commit_menu = None;
            cx.notify();
        });

        if let Some(pos) = self.more_menu {
            let panel = crate::menu::panel(&t, 220.)
                .id("scm-more-menu")
                .on_mouse_down_out(close)
                .child(section("Commit"))
                .child(entry("m-commit", "Commit", Box::new(GitCommit), cx))
                .child(entry("m-amend", "Commit (Amend)", Box::new(GitCommitAmend), cx))
                .child(entry("m-undo", "Undo Last Commit", Box::new(GitUndoLastCommit), cx))
                .child(entry("m-revert", "Revert Commit…", Box::new(GitRevertCommit), cx))
                .child(crate::menu::separator(&t))
                .child(section("Changes"))
                .child(entry("m-stage-all", "Stage All Changes", Box::new(GitStageAll), cx))
                .child(entry("m-unstage-all", "Unstage All Changes", Box::new(GitUnstageAll), cx))
                .child(entry("m-discard-all", "Discard All Changes", Box::new(GitDiscardAll), cx))
                .child(crate::menu::separator(&t))
                .child(section("Stash"))
                .child(entry("m-stash", "Stash Changes…", Box::new(GitStash), cx))
                .child(entry("m-stash-pop-latest", "Pop Latest Stash", Box::new(GitStashPopLatest), cx))
                .child(entry("m-stash-pop", "Pop Stash…", Box::new(GitStashPop), cx))
                .child(entry("m-stash-apply", "Apply Stash…", Box::new(GitStashApply), cx))
                .child(entry("m-stash-drop", "Drop Stash…", Box::new(GitStashDrop), cx))
                .child(crate::menu::separator(&t))
                .child(section("Branch"))
                .child(entry("m-checkout", "Checkout to…", Box::new(GitCheckout), cx))
                .child(entry("m-create-branch", "Create Branch…", Box::new(GitCreateBranch), cx))
                .child(entry("m-rename-branch", "Rename Branch…", Box::new(GitRenameBranch), cx))
                .child(entry("m-merge", "Merge Branch…", Box::new(GitMergeBranch), cx))
                .child(entry("m-abort-merge", "Abort Merge", Box::new(GitAbortMerge), cx))
                .child(crate::menu::separator(&t))
                .child(section("Remote"))
                .child(entry("m-pull", "Pull", Box::new(GitPull), cx))
                .child(entry("m-push", "Push", Box::new(GitPush), cx))
                .child(entry("m-fetch", "Fetch", Box::new(GitFetch), cx))
                .child(entry("m-sync", "Sync (Pull, then Push)", Box::new(GitSync), cx))
                .child(crate::menu::separator(&t))
                .child(section("Tags"))
                .child(entry("m-tag", "Create Tag…", Box::new(GitCreateTag), cx))
                .child(entry("m-push-tags", "Push Tags", Box::new(GitPushTags), cx));
            return Some(gpui::deferred(gpui::anchored().position(pos).snap_to_window().child(panel)).with_priority(1).into_any_element());
        }

        let (commit, pos) = self.commit_menu.clone()?;
        let is_head = self.status.head.as_deref() == Some(commit.id.as_str());
        let (id1, id2, id3) = (commit.id.clone(), commit.id.clone(), commit.id.clone());
        let subject = commit.subject.clone();
        let panel = crate::menu::panel(&t, 200.)
            .id("scm-commit-menu")
            .on_mouse_down_out(close)
            .child(crate::menu::item(&t, "c-copy-id", "Copy Commit ID", None).on_click(cx.listener(move |v, _, _, cx| {
                v.commit_menu = None;
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(id1.clone()));
                cx.notify();
            })))
            .child(crate::menu::item(&t, "c-copy-message", "Copy Commit Message", None).on_click(cx.listener(move |v, _, _, cx| {
                v.commit_menu = None;
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(subject.clone()));
                cx.notify();
            })))
            .child(crate::menu::separator(&t))
            .child(entry("c-tag", "Create Tag Here…", Box::new(GitCreateTagAt { id: id2 }), cx))
            .child(entry("c-revert", "Revert Commit", Box::new(GitRevertCommitId { id: id3, subject: commit.subject.clone() }), cx))
            .when(is_head, |d| d.child(entry("c-undo", "Undo Commit", Box::new(GitUndoLastCommit), cx)));
        Some(gpui::deferred(gpui::anchored().position(pos).snap_to_window().child(panel)).with_priority(1).into_any_element())
    }
}

/// Row action buttons, shown over the right end of a row (left of the status letter) only while it is hovered,
/// so they never take space away from the file name.
fn hover_actions(t: &Theme) -> gpui::Div {
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .right(px(30.))
        .bg(t.bg("sideBar.background"))
        .child(div().absolute().inset_0().bg(t.bg("list.hoverBackground")))
        .flex()
        .items_center()
        .gap_1()
        .px_1()
        .invisible()
        .group_hover("scm-row", |s| s.visible())
}

/// Pull / push / fetch buttons for the HISTORY header (like the Graph header in VS Code), with the number of
/// incoming and outgoing commits. An unpublished branch gets a single Publish button.
fn remote_actions(view: &ScmView, t: &Theme, entity: &Entity<ScmView>) -> Vec<gpui::AnyElement> {
    let s = &view.status;
    if !s.has_commits {
        return Vec::new();
    }
    let busy = view.busy.is_some();
    let button = |id: &'static str, name: &'static str, count: u32, tip: String, run: fn(&mut ScmView, &mut Context<ScmView>)| {
        let e = entity.clone();
        div()
            .id(id)
            .px(px(3.))
            .py(px(2.))
            .flex()
            .items_center()
            .gap(px(2.))
            .rounded(px(3.))
            .cursor_pointer()
            .font_weight(gpui::FontWeight::NORMAL)
            .text_color(t.c("foreground.muted"))
            .hover(|st| st.bg(t.c("toolbar.hoverBackground")))
            .when(busy, |d| d.opacity(0.5))
            .tooltip(tooltip(tip))
            .child(icon(name, px(14.), t.c("icon.foreground")))
            .when(count > 0, |d| d.child(count.to_string()))
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                e.update(cx, run)
            })
            .into_any_element()
    };
    if s.upstream.is_none() {
        return vec![button("history-publish", "arrowUp", 0, "Publish Branch to origin".into(), ScmView::push)];
    }
    let incoming = if s.behind > 0 { format!("Pull {} incoming commit(s) (fast-forward)", s.behind) } else { "Pull (fast-forward)".into() };
    let outgoing = if s.ahead > 0 { format!("Push {} outgoing commit(s)", s.ahead) } else { "Push".into() };
    vec![
        button("history-fetch", "sync", 0, "Fetch (update incoming commits, don't change any files)".into(), ScmView::fetch),
        button("history-pull", "arrowDown", s.behind, incoming, ScmView::pull),
        button("history-push", "arrowUp", s.ahead, outgoing, ScmView::push),
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn short_ages() {
        assert_eq!(super::short_age("14 minutes ago"), "14m");
        assert_eq!(super::short_age("1 hour ago"), "1h");
        assert_eq!(super::short_age("2 years, 3 months ago"), "2y");
        assert_eq!(super::short_age("yesterday"), "yesterday");
    }
}
