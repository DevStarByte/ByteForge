//! The merge editor for a file with conflicts: the two sides as git has them, Current (ours) and
//! Incoming (theirs), read-only next to each other, and below them the Result, which is the file's own
//! editor (so typing, undo, saving and the language server work as always). The toolbar counts the
//! conflicts left, steps through them, accepts a side for the one at the caret and marks the file
//! resolved.

use std::path::PathBuf;

use bf_core::conflicts::Resolution;
use bf_syntax::Language;
use gpui::{Context, Entity, EventEmitter, IntoElement, Render, Subscription, Window, div, prelude::*, px};

use crate::assets::icon;
use crate::editor::{EditorEvent, EditorView};
use crate::theme::Theme;

pub enum MergeEvent {
    /// "Mark as Resolved": save the file and stage it.
    Resolve(PathBuf),
    /// Back to the plain editor.
    Close,
}

pub struct MergeView {
    /// The file's editor.
    result: Entity<EditorView>,
    current: Entity<EditorView>,
    incoming: Entity<EditorView>,
    file: PathBuf,
    /// What the sides are called (the branch names of the conflict markers, when known).
    labels: (String, String),
    _sub: Subscription,
}

impl EventEmitter<MergeEvent> for MergeView {}

impl MergeView {
    /// `result` is the file's editor; `current` and `incoming` are the file's sides from git (`None`: the
    /// file doesn't exist on that side).
    pub fn new(result: Entity<EditorView>, file: PathBuf, current: Option<String>, incoming: Option<String>, cx: &mut Context<Self>) -> Self {
        let language = Language::from_path(&file);
        let side = |text: Option<String>, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut e = EditorView::new(text.as_deref().unwrap_or("(the file doesn't exist on this side)"), None, cx);
                e.read_only = true;
                e.minimap_enabled = false;
                e.folding_enabled = false;
                e.set_language(language, cx);
                e
            })
        };
        let (current, incoming) = (side(current, cx), side(incoming, cx));
        let names = result.read(cx).conflict_summary().1;
        let labels = names.unwrap_or_else(|| ("HEAD".into(), "incoming".into()));
        // The count in the toolbar follows the edits.
        let sub = cx.subscribe(&result, |_, _, event: &EditorEvent, cx| {
            if matches!(event, EditorEvent::Edited | EditorEvent::CaretMoved) {
                cx.notify();
            }
        });
        Self { result, current, incoming, file, labels, _sub: sub }
    }

    pub fn conflicts_left(&self, cx: &gpui::App) -> usize {
        self.result.read(cx).conflict_summary().0
    }

    fn accept(&mut self, how: Resolution, window: &mut Window, cx: &mut Context<Self>) {
        self.result.update(cx, |e, cx| {
            // On no conflict: go to the next one first, so the button always does something useful.
            e.accept_conflict(how, cx);
            e.focus(window);
        });
        cx.notify();
    }

    fn step(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.result.update(cx, |e, cx| {
            e.go_to_conflict(forward, cx);
            e.focus(window);
        });
    }
}

impl Render for MergeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::global(cx);
        let (fg, muted, border) = (t.c("breadcrumb.foreground"), t.c("foreground.muted"), t.c("breadcrumb.border"));
        let hover = t.c("toolbar.hoverBackground");
        let left = self.conflicts_left(cx);
        let name = self.file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let text_button = |id: &'static str, label: &'static str| div().id(id).px(px(6.)).py(px(1.)).rounded(px(4.)).cursor_pointer().hover(move |s| s.bg(hover)).child(label);
        let icon_button = |id: &'static str, glyph: &'static str, tip: &'static str| {
            div().id(id).p(px(3.)).rounded(px(4.)).cursor_pointer().hover(move |s| s.bg(hover)).child(icon(glyph, px(14.), fg)).tooltip(crate::workbench::tooltip(tip))
        };
        let status = match left {
            0 => "No conflicts left".to_owned(),
            1 => "1 conflict left".to_owned(),
            n => format!("{n} conflicts left"),
        };
        let resolved_color = t.c_or("gitDecoration.addedResourceForeground", 0x3fb950ff);
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
            .child(div().whitespace_nowrap().child(format!("Merging {name}")))
            .child(div().text_color(if left == 0 { resolved_color } else { muted }).whitespace_nowrap().child(status))
            .child(div().flex_1())
            .when(left > 0, |d| {
                d.child(icon_button("merge-prev", "arrow-up", "Previous Conflict").on_click(cx.listener(|v, _, w, cx| v.step(false, w, cx))))
                    .child(icon_button("merge-next", "arrow-down", "Next Conflict").on_click(cx.listener(|v, _, w, cx| v.step(true, w, cx))))
                    .child(text_button("merge-current", "Accept Current").on_click(cx.listener(|v, _, w, cx| v.accept(Resolution::Current, w, cx))))
                    .child(text_button("merge-incoming", "Accept Incoming").on_click(cx.listener(|v, _, w, cx| v.accept(Resolution::Incoming, w, cx))))
                    .child(text_button("merge-both", "Accept Both").on_click(cx.listener(|v, _, w, cx| v.accept(Resolution::Both, w, cx))))
            })
            .when(left == 0, |d| {
                let file = self.file.clone();
                d.child(text_button("merge-resolve", "Mark as Resolved").text_color(resolved_color).on_click(cx.listener(move |_, _, _, cx| cx.emit(MergeEvent::Resolve(file.clone())))))
            })
            .child(text_button("merge-close", "Close Merge Editor").on_click(cx.listener(|_, _, _, cx| cx.emit(MergeEvent::Close))));
        let header = |label: String, color: gpui::Hsla| div().flex_none().px_2().py(px(2.)).text_size(px(11.)).text_color(color).border_b_1().border_color(border).whitespace_nowrap().child(label);
        let current_color = t.c_or("merge.currentHeaderForeground", 0x3fb950ff);
        let incoming_color = t.c_or("merge.incomingHeaderForeground", 0x58a6ffff);
        let pane = |label: String, color: gpui::Hsla, editor: Entity<EditorView>| div().flex_1().min_w_0().h_full().flex().flex_col().child(header(label, color)).child(div().flex_1().min_h_0().child(editor));
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(toolbar)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .border_b_1()
                    .border_color(border)
                    .child(pane(format!("Current ({})", self.labels.0), current_color, self.current.clone()))
                    .child(div().w(px(1.)).h_full().bg(border))
                    .child(pane(format!("Incoming ({})", self.labels.1), incoming_color, self.incoming.clone())),
            )
            .child(div().flex_1().min_h_0().flex().flex_col().child(header(format!("Result: {name}"), muted)).child(div().flex_1().min_h_0().child(self.result.clone())))
    }
}

#[cfg(test)]
impl MergeView {
    /// The texts of the two sides (current, incoming) for tests.
    pub(crate) fn sides_for_test(&self, cx: &gpui::App) -> (String, String) {
        (self.current.read(cx).state.buffer.text(), self.incoming.read(cx).state.buffer.text())
    }
}
