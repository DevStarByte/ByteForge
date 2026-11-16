//! The diff editor: two editors side by side (old left, new right) whose rows line up and that scroll
//! together, or one read-only inline editor with the removed lines above the added ones. Both highlight
//! the file's syntax, number the lines as in the files, and mark the changed words inside changed lines.
//! A toolbar shows the change counts and steps through the changes (F7 / Shift+F7).
//!
//! Side by side, each editor holds its file's own text; the rows that keep the sides aligned are spacer
//! rows, not text. So the working-tree side of a working-tree diff can be edited in place: the diff
//! follows the typing, and Ctrl+S saves the file.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bf_core::diff::{Diff, Pane, RowKind, SideLayout};
use bf_syntax::Language;
use bf_text::Point;
use gpui::{Context, Entity, EventEmitter, IntoElement, Render, Subscription, Window, div, prelude::*, px};

use crate::actions::{DiffNextChange, DiffPreviousChange, DiffToggleInline};
use crate::assets::icon;
use crate::editor::{EditorEvent, EditorView, ScrollLink};
use crate::theme::Theme;

pub enum DiffEvent {
    /// "Open File": the new side's file, at this line.
    OpenFile(PathBuf, Point),
    /// Stage, unstage or revert the change the toolbar is on, in this file.
    Change(ChangeAction, PathBuf, bf_core::diff::LineChange),
    /// Inline can't show unsaved edits of the working-tree side: save the file, then switch to inline.
    SaveThenInline,
}

/// What the two sides are, which decides what can be done with one change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DiffKind {
    /// Index ↔ working tree: a change can be staged or reverted.
    WorkingTree,
    /// HEAD ↔ index: a change can be unstaged.
    Staged,
    /// A commit, or anything else that's history.
    #[default]
    ReadOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeAction {
    Stage,
    Unstage,
    Revert,
}

pub struct DiffView {
    /// The new side, and the inline view (the tab's editor: Find and focus work on it).
    right: Entity<EditorView>,
    left: Entity<EditorView>,
    diff: Diff,
    inline: bool,
    /// The change the toolbar is on (`None` before stepping).
    current: Option<usize>,
    /// What the sides are ("HEAD", "Working Tree", a commit).
    labels: (String, String),
    /// The file to open, when the new side is the working tree or a file that exists.
    file: Option<PathBuf>,
    language: Option<Language>,
    kind: DiffKind,
    /// Long unchanged stretches are folded away ("Collapse Unchanged").
    collapsed: bool,
    /// The texts compared (`new` follows the edits of the working-tree side).
    old: String,
    new: String,
    /// The sides as their own lines with spacers (side by side).
    left_layout: SideLayout,
    right_layout: SideLayout,
    _sub: Subscription,
}

impl EventEmitter<DiffEvent> for DiffView {}

impl DiffView {
    /// `right` becomes the new side (the tab keeps it as its editor).
    pub fn new(right: Entity<EditorView>, language: Option<Language>, cx: &mut Context<Self>) -> Self {
        let link: ScrollLink = Default::default();
        let left = cx.new(|cx| EditorView::new("", None, cx));
        for editor in [&left, &right] {
            editor.update(cx, |e, cx| {
                e.read_only = true;
                e.folding_enabled = false; // folds would put the sides out of step
                e.minimap_enabled = false;
                e.scroll_link = Some(link.clone());
                e.set_language(language, cx);
            });
        }
        left.update(cx, |e, _| e.spacer_background = Some("diffEditor.diagonalFill"));
        right.update(cx, |e, _| e.spacer_background = Some("diffEditor.diagonalFill"));
        // Typing in the working-tree side: compare again.
        let sub = cx.subscribe(&right, |v: &mut Self, _, event: &EditorEvent, cx| {
            if matches!(event, EditorEvent::Edited) {
                v.on_right_edited(cx);
            }
        });
        Self {
            right,
            left,
            diff: Diff::default(),
            inline: false,
            current: None,
            labels: Default::default(),
            file: None,
            language,
            kind: DiffKind::ReadOnly,
            collapsed: false,
            old: String::new(),
            new: String::new(),
            left_layout: SideLayout::default(),
            right_layout: SideLayout::default(),
            _sub: sub,
        }
    }

    /// The working-tree side can be edited (side by side, in a working-tree diff of a file that exists).
    fn editable(&self) -> bool {
        self.kind == DiffKind::WorkingTree && self.file.is_some() && !self.inline
    }

    /// The file the new side is, while it can be edited (saving the tab writes it).
    pub fn editable_file(&self) -> Option<&Path> {
        self.file.as_deref().filter(|_| self.editable())
    }

    /// Shows the comparison of `old` and `new`; keeps the view mode (and the change, when there still is one).
    /// Unsaved edits of the working-tree side stay: they're compared instead of `new`.
    pub fn set_texts(&mut self, old: &str, new: &str, labels: (String, String), file: Option<PathBuf>, kind: DiffKind, cx: &mut Context<Self>) {
        let keep_edits = self.editable() && kind == DiffKind::WorkingTree && self.right.read(cx).is_modified();
        self.old = old.to_owned();
        self.new = if keep_edits { self.right.read(cx).state.buffer.text() } else { new.to_owned() };
        self.diff = bf_core::diff::compare(&self.old, &self.new);
        self.labels = labels;
        self.file = file;
        self.kind = kind;
        self.current = self.current.filter(|&c| c < self.diff.hunks.len());
        self.refresh(cx);
        if self.current.is_none() && !self.diff.is_unchanged() {
            // Start at the first change, like Visual Studio and VS Code.
            self.go_to_change(0, cx);
        }
    }

    pub fn set_language(&mut self, language: Option<Language>, cx: &mut Context<Self>) {
        self.language = language;
        for editor in [&self.left, &self.right] {
            editor.update(cx, |e, cx| e.set_language(language, cx));
        }
    }

    /// The working-tree side changed: compare its text again (keeping the change the toolbar is on).
    fn on_right_edited(&mut self, cx: &mut Context<Self>) {
        if !self.editable() {
            return;
        }
        let text = self.right.read(cx).state.buffer.text();
        if text == self.new {
            return;
        }
        self.new = text;
        self.diff = bf_core::diff::compare(&self.old, &self.new);
        self.current = self.current.filter(|&c| c < self.diff.hunks.len());
        self.refresh(cx);
    }

    /// Loads the texts and decorations into the editors for the current mode.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let editable = self.editable();
        self.right.update(cx, |e, _| e.read_only = !editable);
        // The same unchanged stretches fold on both sides, so they stay in step.
        let fold_rows = |kinds: &[RowKind]| if self.collapsed { bf_core::diff::unchanged_folds(kinds, 3) } else { Vec::new() };
        if self.inline {
            let pane = &self.diff.inline;
            let folds = fold_rows(&pane.kinds);
            self.right.update(cx, |e, cx| {
                e.spacers = None;
                show_pane(e, pane, cx);
                e.folds.set_folded(folds);
            });
        } else {
            self.left_layout = SideLayout::of(&self.diff.left);
            self.right_layout = SideLayout::of(&self.diff.right);
            let rows = fold_rows(&self.diff.left.kinds);
            for (editor, text, layout) in [(&self.left, &self.old, &self.left_layout), (&self.right, &self.new, &self.right_layout)] {
                let folds = layout.line_ranges(&rows);
                editor.update(cx, |e, cx| {
                    // Only when it differs: reloading drops the undo history and moves the caret.
                    if e.state.buffer.text() != *text {
                        e.reload_text(text, cx);
                    }
                    show_side(e, layout, cx);
                    e.folds.set_folded(folds);
                });
            }
        }
        cx.notify();
    }

    /// "Collapse Unchanged" / "Show All": hides the long stretches without changes.
    pub fn toggle_collapsed(&mut self, cx: &mut Context<Self>) {
        self.collapsed = !self.collapsed;
        self.refresh(cx);
        if let Some(c) = self.current {
            self.go_to_change(c, cx);
        }
    }

    /// Stage, unstage or revert the change the toolbar is on (the workbench does it and reloads the diff).
    pub(crate) fn change_action(&mut self, action: ChangeAction, cx: &mut Context<Self>) {
        let (Some(file), Some(change)) = (self.file.clone(), self.current.and_then(|c| self.diff.changes.get(c).copied())) else { return };
        cx.emit(DiffEvent::Change(action, file, change));
    }

    /// The line of the new side (the inline text, inline) each change starts on.
    fn hunk_rows(&self) -> Vec<usize> {
        if self.inline { self.diff.inline_hunks.clone() } else { self.diff.hunks.iter().map(|&r| self.right_layout.row_lines.get(r).copied().unwrap_or(0)).collect() }
    }

    fn go_to_change(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(&line) = self.hunk_rows().get(ix) else { return };
        self.current = Some(ix);
        // Both sides follow: the scroll link moves the other one.
        self.right.update(cx, |e, cx| e.go_to(Point::new(line, 0), cx));
        if !self.inline {
            let row = self.diff.hunks[ix];
            let left = self.left_layout.row_lines.get(row).copied().unwrap_or(0);
            self.left.update(cx, |e, _| e.state.set_caret(Point::new(left, 0), false));
        }
        cx.notify();
    }

    /// Steps to the next (or previous) change after the caret, wrapping around at the ends.
    pub fn step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let caret = self.right.read(cx).state.caret().line;
        let rows = self.hunk_rows();
        if rows.is_empty() {
            return;
        }
        let ix = if forward { rows.iter().position(|&r| r > caret).unwrap_or(0) } else { rows.iter().rposition(|&r| r < caret).unwrap_or(rows.len() - 1) };
        self.go_to_change(ix, cx);
    }

    pub fn toggle_inline(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editable() && self.right.read(cx).is_modified() {
            cx.emit(DiffEvent::SaveThenInline);
            return;
        }
        self.inline = !self.inline;
        self.refresh(cx);
        if let Some(c) = self.current {
            self.go_to_change(c, cx);
        }
        self.right.update(cx, |e, _| e.focus(window));
    }

    fn open_file(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.file.clone() else { return };
        let line = self.current.and_then(|c| self.diff.hunk_lines.get(c).copied()).unwrap_or(0);
        cx.emit(DiffEvent::OpenFile(path, Point::new(line, 0)));
    }
}

/// One side, side by side: the file's own lines, spacers between them, its changes marked.
fn show_side(e: &mut EditorView, layout: &SideLayout, cx: &mut Context<EditorView>) {
    e.line_numbers = None;
    e.spacers = Some(Arc::new(layout.spacers.clone()));
    e.line_backgrounds = layout
        .kinds
        .iter()
        .enumerate()
        .filter_map(|(line, kind)| match kind {
            RowKind::Removed => Some((line, "diffEditor.removedLineBackground")),
            RowKind::Added => Some((line, "diffEditor.insertedLineBackground")),
            _ => None,
        })
        .collect();
    e.range_backgrounds = layout
        .words
        .iter()
        .map(|&(line, s, end)| {
            let key = if layout.kinds.get(line) == Some(&RowKind::Removed) { "diffEditor.removedTextBackground" } else { "diffEditor.insertedTextBackground" };
            (Point::new(line, s), Point::new(line, end), key)
        })
        .collect();
    cx.notify();
}

fn show_pane(e: &mut EditorView, pane: &Pane, cx: &mut Context<EditorView>) {
    e.reload_text(&pane.text, cx);
    e.line_numbers = Some(Arc::new(pane.numbers.clone()));
    e.line_backgrounds = pane
        .kinds
        .iter()
        .enumerate()
        .filter_map(|(row, kind)| {
            let key = match kind {
                RowKind::Removed => "diffEditor.removedLineBackground",
                RowKind::Added => "diffEditor.insertedLineBackground",
                RowKind::Filler => "diffEditor.diagonalFill",
                RowKind::Unchanged => return None,
            };
            Some((row, key))
        })
        .collect();
    e.range_backgrounds = pane
        .words
        .iter()
        .map(|&(row, s, end)| {
            let key = if pane.kinds.get(row) == Some(&RowKind::Removed) { "diffEditor.removedTextBackground" } else { "diffEditor.insertedTextBackground" };
            (Point::new(row, s), Point::new(row, end), key)
        })
        .collect();
    cx.notify();
}

impl Render for DiffView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::global(cx);
        let muted = t.c("foreground.muted");
        let fg = t.c("breadcrumb.foreground");
        let hover = t.c("toolbar.hoverBackground");
        let border = t.c("breadcrumb.border");
        let n = self.hunk_rows().len();
        let button = |id: &'static str, glyph: &'static str, tip: &'static str| {
            div()
                .id(id)
                .p(px(3.))
                .rounded(px(4.))
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .child(icon(glyph, px(14.), fg))
                .tooltip(crate::workbench::tooltip(tip))
        };
        let text_button = |id: &'static str, label: &'static str| {
            div().id(id).px(px(6.)).py(px(1.)).rounded(px(4.)).cursor_pointer().hover(move |s| s.bg(hover)).child(label)
        };
        let position = match (self.current, n) {
            (_, 0) => "No changes".to_owned(),
            (Some(c), n) => format!("Change {} of {n}", c + 1),
            (None, n) => format!("{n} change{}", if n == 1 { "" } else { "s" }),
        };
        let toolbar = div()
            .flex_none()
            .h(t.m("breadcrumb.height", 22.0) + px(4.))
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .bg(t.bg("breadcrumb.background"))
            .border_b_1()
            .border_color(border)
            .text_size(t.m("breadcrumb.fontSize", 12.0))
            .text_color(fg)
            .child(div().text_color(muted).whitespace_nowrap().overflow_hidden().child(format!("{}  \u{2194}  {}", self.labels.0, self.labels.1)))
            .child(div().flex_1())
            .child(div().text_color(t.c_or("gitDecoration.addedResourceForeground", 0x3fb950ff)).child(format!("+{}", self.diff.added)))
            .child(div().text_color(t.c_or("gitDecoration.deletedResourceForeground", 0xf85149ff)).child(format!("\u{2212}{}", self.diff.removed)))
            .child(div().text_color(muted).whitespace_nowrap().child(position))
            .child(button("diff-prev", "arrow-up", "Previous Change (Shift+F7)").on_click(cx.listener(|v, _, _, cx| v.step(false, cx))))
            .child(button("diff-next", "arrow-down", "Next Change (F7)").on_click(cx.listener(|v, _, _, cx| v.step(true, cx))))
            .when(self.current.is_some() && self.file.is_some() && self.kind == DiffKind::WorkingTree, |d| {
                d.child(text_button("diff-stage", "Stage Change").on_click(cx.listener(|v, _, _, cx| v.change_action(ChangeAction::Stage, cx))))
                    .child(text_button("diff-revert", "Revert Change").on_click(cx.listener(|v, _, _, cx| v.change_action(ChangeAction::Revert, cx))))
            })
            .when(self.current.is_some() && self.file.is_some() && self.kind == DiffKind::Staged, |d| {
                d.child(text_button("diff-unstage", "Unstage Change").on_click(cx.listener(|v, _, _, cx| v.change_action(ChangeAction::Unstage, cx))))
            })
            .child(text_button("diff-collapse", if self.collapsed { "Show All" } else { "Collapse Unchanged" }).on_click(cx.listener(|v, _, _, cx| v.toggle_collapsed(cx))))
            .child(text_button("diff-mode", if self.inline { "Side by Side" } else { "Inline" }).on_click(cx.listener(|v, _, w, cx| v.toggle_inline(w, cx))))
            .when(self.file.is_some(), |d| d.child(text_button("diff-open", "Open File").on_click(cx.listener(|v, _, _, cx| v.open_file(cx)))));
        let body = if self.inline {
            div().flex_1().min_h_0().child(self.right.clone())
        } else {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(div().flex_1().min_w_0().h_full().border_r_1().border_color(border).child(self.left.clone()))
                .child(div().flex_1().min_w_0().h_full().child(self.right.clone()))
        };
        div()
            .key_context("DiffView")
            .size_full()
            .flex()
            .flex_col()
            .on_action(cx.listener(|v, _: &DiffNextChange, _, cx| v.step(true, cx)))
            .on_action(cx.listener(|v, _: &DiffPreviousChange, _, cx| v.step(false, cx)))
            .on_action(cx.listener(|v, _: &DiffToggleInline, w, cx| v.toggle_inline(w, cx)))
            .child(toolbar)
            .child(body)
    }
}

#[cfg(test)]
impl DiffView {
    /// (changes, the change the toolbar is on, rows hidden on the new side) for tests.
    pub(crate) fn state_for_test(&self, cx: &gpui::App) -> (usize, Option<usize>, usize) {
        let hidden = self.right.read(cx).folds.hidden_ranges().iter().map(|(s, e)| e + 1 - s).sum();
        (self.diff.changes.len(), self.current, hidden)
    }

    /// Spacer rows on (left, right), side by side.
    pub(crate) fn spacer_rows_for_test(&self, cx: &gpui::App) -> (usize, usize) {
        let sum = |e: &Entity<EditorView>| e.read(cx).spacers.as_ref().map_or(0, |s| s.iter().sum());
        (sum(&self.left), sum(&self.right))
    }
}
