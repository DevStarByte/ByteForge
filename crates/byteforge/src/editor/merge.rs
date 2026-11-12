//! Git in the text: the blame of the caret's line, drawn after it ("Ann, 3 days ago • Fix the parser"),
//! and merge conflicts: their sides colored, a hint on each `<<<<<<<` line, and Accept Current /
//! Incoming / Both.

use bf_core::conflicts::{self, Conflict, Resolution};
use bf_core::git::{BlameLine, Git};

use super::*;

/// Code-action kinds of the conflict entries in the Ctrl+. menu.
pub(super) const ACCEPT_KINDS: [(&str, &str, Resolution); 3] = [
    ("byteforge.merge.current", "Accept Current Change", Resolution::Current),
    ("byteforge.merge.incoming", "Accept Incoming Change", Resolution::Incoming),
    ("byteforge.merge.both", "Accept Both Changes", Resolution::Both),
];

/// Blame of the open file and the task refreshing it.
#[derive(Default)]
pub(crate) struct BlameState {
    pub repo: Option<Git>,
    /// The current-line annotation ("git.blame.currentLine").
    pub enabled: bool,
    /// The blame column in the gutter ("Git: Toggle Blame Gutter").
    pub gutter: bool,
    lines: Arc<Vec<BlameLine>>,
    /// The text changed since the blame: lines may have moved, so nothing is shown until it is redone.
    stale: bool,
    task: Option<Task<()>>,
}

impl EditorView {
    /// The repository the file belongs to (`None`: not in one), and whether to annotate the caret's line.
    pub fn set_git_repo(&mut self, repo: Option<Git>, blame: bool, cx: &mut Context<Self>) {
        self.blame.repo = repo;
        self.blame.enabled = blame;
        self.schedule_blame(Duration::ZERO, cx);
    }

    pub fn set_blame_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.blame.enabled = enabled;
        self.schedule_blame(Duration::ZERO, cx);
        cx.notify();
    }

    /// Re-blames after `delay` (edits wait a little, so typing doesn't run git on every key).
    pub(super) fn schedule_blame(&mut self, delay: Duration, cx: &mut Context<Self>) {
        let (Some(repo), Some(path)) = (self.blame.repo.clone(), self.path.clone()) else { return };
        if !(self.blame.enabled || self.blame.gutter) || self.read_only {
            return;
        }
        let rope = self.state.buffer.rope().clone();
        let version = self.state.buffer.version();
        self.blame.task = Some(cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let result = cx.background_executor().spawn(async move { repo.blame(&path, &rope.to_string()) }).await;
            let _ = this.update(cx, |view, cx| {
                if view.state.buffer.version() != version {
                    return;
                }
                // Not committed at all (new file, or git can't blame it): no annotation.
                view.blame.lines = Arc::new(result.unwrap_or_default());
                view.blame.stale = false;
                cx.notify();
            });
        }));
    }

    /// Shows or hides the blame column in the gutter.
    pub fn set_blame_gutter(&mut self, on: bool, cx: &mut Context<Self>) {
        self.blame.gutter = on;
        if on {
            self.schedule_blame(Duration::ZERO, cx);
        }
        cx.notify();
    }

    /// Whether the blame column shows (it needs a repository).
    pub fn blame_gutter(&self) -> bool {
        self.blame.gutter && self.blame.repo.is_some() && self.path.is_some()
    }

    /// The blame column's text for `line`: its commit, author and age, or nothing on the lines that
    /// continue the commit of the line above (`first_visible` always gets its text).
    pub(crate) fn blame_gutter_label(&self, line: usize, first_visible: usize, now: i64) -> Option<String> {
        if self.blame.stale {
            return None;
        }
        let b = self.blame.lines.get(line)?;
        let continues = line > first_visible && line > 0 && self.blame.lines.get(line - 1).is_some_and(|above| above.commit == b.commit);
        (!continues).then(|| b.gutter_label(now))
    }

    /// Text edited: blame and conflicts move with it.
    pub(super) fn after_edit_merge(&mut self, cx: &mut Context<Self>) {
        if !self.blame.lines.is_empty() {
            self.blame.stale = true;
        }
        self.schedule_blame(Duration::from_millis(1200), cx);
        self.refresh_conflicts();
    }

    /// The blame of `line`, when it is current.
    pub fn blame_of(&self, line: usize) -> Option<&BlameLine> {
        if self.blame.stale || !self.blame.enabled {
            return None;
        }
        self.blame.lines.get(line)
    }

    /// Text drawn after the end of `line`: the conflict actions on a `<<<<<<<` line, else the blame of the
    /// caret's line.
    pub(crate) fn end_of_line_note(&self, line: usize) -> Option<String> {
        if self.conflicts.iter().any(|c| c.start == line) {
            return Some(format!("Accept Current \u{00b7} Accept Incoming \u{00b7} Accept Both ({})", crate::actions::key_label("ctrl-.")));
        }
        if line != self.state.caret().line || self.state.has_selection() || self.state.has_multiple_cursors() {
            return None;
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        self.blame_of(line).map(|b| b.label(now))
    }

    // ───────────── merge conflicts ─────────────

    /// Finds the conflicts again and colors their sides (editable files only; diff panes use the same
    /// backgrounds for themselves).
    pub(super) fn refresh_conflicts(&mut self) {
        if self.read_only {
            return;
        }
        let had = !self.conflicts.is_empty();
        self.conflicts = conflicts::find(&self.state.buffer.text());
        if !had && self.conflicts.is_empty() {
            return;
        }
        let mut backgrounds = Vec::new();
        for c in &self.conflicts {
            backgrounds.push((c.start, "merge.currentHeaderBackground"));
            for l in c.start + 1..c.base.unwrap_or(c.separator) {
                backgrounds.push((l, "merge.currentContentBackground"));
            }
            if let Some(base) = c.base {
                for l in base..c.separator {
                    backgrounds.push((l, "merge.commonContentBackground"));
                }
            }
            for l in c.separator + 1..c.end {
                backgrounds.push((l, "merge.incomingContentBackground"));
            }
            backgrounds.push((c.end, "merge.incomingHeaderBackground"));
        }
        self.line_backgrounds = backgrounds;
    }

    /// The conflict around the caret.
    fn conflict_at_caret(&self) -> Option<&Conflict> {
        let line = self.state.caret().line;
        self.conflicts.iter().find(|c| c.start <= line && line <= c.end)
    }

    /// The Ctrl+. menu's entries for the conflict at the caret.
    pub(super) fn conflict_entries(&self) -> Vec<bf_lsp::CodeAction> {
        let Some(c) = self.conflict_at_caret() else { return Vec::new() };
        ACCEPT_KINDS
            .iter()
            .map(|(kind, title, how)| {
                let detail = match how {
                    Resolution::Current => format!(" ({})", c.current_label),
                    Resolution::Incoming => format!(" ({})", c.incoming_label),
                    Resolution::Both => String::new(),
                };
                bf_lsp::CodeAction {
                    title: format!("{title}{}", if detail == " ()" { "" } else { &detail }),
                    kind: Some((*kind).into()),
                    preferred: false,
                    disabled: None,
                    edit: None,
                    command: None,
                    raw: serde_json::Value::Null,
                }
            })
            .collect()
    }

    /// Keeps one side (or both) of the conflict at the caret, as one undo step.
    pub fn accept_conflict(&mut self, how: Resolution, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let Some(c) = self.conflict_at_caret().cloned() else {
            cx.emit(EditorEvent::Log("Put the caret inside a merge conflict (between <<<<<<< and >>>>>>>).".into()));
            return;
        };
        let new = conflicts::apply(&self.state.buffer.text(), &c, how);
        // One undo step that only touches the conflict's lines.
        self.replace_text_minimal(&new, cx);
        self.state.set_caret(self.state.buffer.clamp(Point::new(c.start, 0)), false);
        self.after_move(cx);
        if self.conflicts.is_empty() {
            let name = self.path.as_deref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            cx.emit(EditorEvent::Log(format!("No conflicts left in {name}. Save it and stage it in Source Control to mark it resolved.")));
        }
        cx.notify();
    }

    /// Goes to the next (or previous) conflict, wrapping around.
    pub fn go_to_conflict(&mut self, forward: bool, cx: &mut Context<Self>) {
        let line = self.state.caret().line;
        let target = if forward {
            self.conflicts.iter().find(|c| c.start > line).or(self.conflicts.first())
        } else {
            self.conflicts.iter().rev().find(|c| c.start < line).or(self.conflicts.last())
        };
        match target.map(|c| c.start) {
            Some(start) => self.go_to(Point::new(start, 0), cx),
            None => cx.emit(EditorEvent::Log("No merge conflicts in this file.".into())),
        }
    }

    /// Runs a conflict entry of the Ctrl+. menu; `false` when it isn't one.
    pub(super) fn run_conflict_entry(&mut self, kind: &str, cx: &mut Context<Self>) -> bool {
        match ACCEPT_KINDS.iter().find(|(k, _, _)| *k == kind) {
            Some((_, _, how)) => {
                self.accept_conflict(*how, cx);
                true
            }
            None => false,
        }
    }

    /// "Git: Show Commit of Line": asks the workbench to open the change that last touched the caret's line.
    pub fn show_line_commit(&mut self, cx: &mut Context<Self>) {
        let line = self.state.caret().line;
        let (Some(repo), Some(path)) = (self.blame.repo.clone(), self.path.clone()) else {
            cx.emit(EditorEvent::Log("This file isn't in a git repository.".into()));
            return;
        };
        // Blame now: the annotation may be off, or the text just changed.
        let text = self.state.buffer.text();
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { repo.blame(&path, &text) }).await;
            let _ = this.update(cx, |_, cx| {
                let event = match result.ok().and_then(|b| b.into_iter().nth(line)) {
                    Some(b) if b.uncommitted => EditorEvent::Log("This line isn't committed yet.".into()),
                    Some(b) => EditorEvent::ShowCommit { commit: b.commit, filename: b.filename },
                    None => EditorEvent::Log("Git has no history for this line (is the file committed?).".into()),
                };
                cx.emit(event);
            });
        })
        .detach();
    }
}
