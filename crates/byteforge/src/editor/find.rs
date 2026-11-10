//! Find and replace, and the find bar.

use super::*;

impl EditorView {
    pub(super) fn open_find(&mut self, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.state.selected_text();
        if self.find.is_none() {
            let query = cx.new(|cx| TextInput::new("Find", cx));
            let replace_input = cx.new(|cx| TextInput::new("Replace", cx));
            let subs = vec![
                cx.subscribe_in(&query, window, |view, _, event, window, cx| match event {
                    InputEvent::Changed => view.update_find(true, cx),
                    InputEvent::Confirmed | InputEvent::Down => view.find_step(true, cx),
                    InputEvent::Up => view.find_step(false, cx),
                    InputEvent::Cancelled => view.close_find(window, cx),
                    _ => {}
                }),
                cx.subscribe_in(&replace_input, window, |view, _, event, window, cx| match event {
                    InputEvent::Confirmed => view.replace_one(cx),
                    InputEvent::Cancelled => view.close_find(window, cx),
                    _ => {}
                }),
            ];
            self.find = Some(FindState { query, replace: replace_input, show_replace: replace, match_case: false, matches: Vec::new(), _subs: subs });
        }
        let find = self.find.as_mut().expect("find state");
        find.show_replace = replace;
        let query = find.query.clone();
        if !selected.is_empty() && !selected.contains('\n') {
            query.update(cx, |q, cx| q.set_text(selected, cx));
        }
        query.update(cx, |q, cx| q.select_all_text(cx));
        window.focus(&query.focus_handle(cx));
        self.update_find(false, cx);
    }

    pub(super) fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find = None;
        window.focus(&self.focus);
        cx.notify();
    }

    pub(super) fn update_find(&mut self, select: bool, cx: &mut Context<Self>) {
        let Some(find) = &self.find else { return };
        let query = find.query.read(cx).text().to_owned();
        let matches = self.state.find_all(&query, find.match_case, 50_000);
        if select && !matches.is_empty() {
            let (start, _) = self.state.selection();
            let m = matches.iter().find(|m| m.0 >= start).copied().unwrap_or(matches[0]);
            self.state.select(m.0, m.1);
            self.reveal(m.0, false);
        }
        if let Some(find) = &mut self.find {
            find.matches = matches;
        }
        cx.notify();
    }

    pub(super) fn find_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(find) = &self.find else { return };
        if find.matches.is_empty() {
            return;
        }
        let (s, e) = self.state.selection();
        let m = if forward {
            find.matches.iter().find(|m| m.0 > s || (m.0 == s && m.1 != e)).copied().unwrap_or(find.matches[0])
        } else {
            find.matches.iter().rev().find(|m| m.0 < s).copied().unwrap_or(*find.matches.last().unwrap())
        };
        self.state.select(m.0, m.1);
        self.reveal(m.0, false);
        cx.notify();
    }

    pub(super) fn replace_one(&mut self, cx: &mut Context<Self>) {
        let Some(find) = &self.find else { return };
        let replacement = find.replace.read(cx).text().to_owned();
        let sel = self.state.selection();
        if find.matches.contains(&sel) {
            self.state.insert(&replacement);
            self.after_edit(cx);
        }
        self.find_step(true, cx);
    }

    pub(super) fn replace_all(&mut self, cx: &mut Context<Self>) {
        let Some(find) = &self.find else { return };
        let replacement = find.replace.read(cx).text().to_owned();
        let matches = find.matches.clone();
        self.state.buffer.undo_stack().begin_group();
        for (s, e) in matches.iter().rev() {
            self.state.buffer.replace(*s, *e, &replacement);
        }
        self.state.buffer.undo_stack().end_group();
        self.after_edit(cx);
        self.update_find(false, cx);
    }

    pub(crate) fn find_matches(&self) -> &[(Point, Point)] {
        self.find.as_ref().map_or(&[], |f| &f.matches)
    }

    // ───────────── scrolling & reveal ─────────────
}

impl EditorView {
    pub(super) fn render_find(&self, find: &FindState, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::global(cx);
        let count = find.matches.len();
        let (s, e) = self.state.selection();
        let index = find.matches.iter().position(|m| *m == (s, e));
        let label = match (find.query.read(cx).text().is_empty(), count, index) {
            (true, _, _) => String::new(),
            (_, 0, _) => "No results".into(),
            (_, n, Some(i)) => format!("{} of {n}", i + 1),
            (_, n, None) => format!("{n} results"),
        };
        let fg = t.c("editorWidget.foreground");
        let fg = if fg.a > 0.0 { fg } else { t.c("window.foreground") };
        let icon_size = t.m("findWidget.iconSize", 14.0);
        let hover_bg = t.c("toolbar.hoverBackground");
        let button = |id: &'static str, name: &'static str| {
            div().id(id).p_1().rounded(px(4.)).cursor_pointer().hover(move |s| s.bg(hover_bg)).child(icon(name, icon_size, fg))
        };
        let match_case = find.match_case;
        div()
            .absolute()
            .top_0()
            .right(px(18.))
            .w(t.m("findWidget.inputWidth", 240.0) + px(200.))
            .bg(t.bg("editorWidget.background"))
            .border_color(t.c("editorWidget.border"))
            .border_1()
            .rounded_b(t.radius("findWidget.radius", 6.0))
            .shadow(t.shadow("widget.shadow"))
            .p(t.edges("findWidget.padding", 6.0).top)
            .flex()
            .flex_col()
            .gap_1()
            .text_size(t.m("ui.size", 13.0))
            .text_color(fg)
            .font_family(t.ui_font.clone())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().flex_1().child(find.query.clone()))
                    .child(
                        div()
                            .id("match-case")
                            .px_1()
                            .rounded(px(3.))
                            .cursor_pointer()
                            .when(match_case, |d| d.bg(t.bg("button.background")).text_color(t.c("button.foreground")))
                            .child("Aa")
                            .on_click(cx.listener(|v, _, _, cx| {
                                if let Some(f) = &mut v.find {
                                    f.match_case = !f.match_case;
                                }
                                v.update_find(true, cx);
                            })),
                    )
                    .child(div().min_w(px(70.)).text_color(t.c("foreground.muted")).child(label))
                    .child(button("find-prev", "chevronUp").on_click(cx.listener(|v, _, _, cx| v.find_step(false, cx))))
                    .child(button("find-next", "chevronDown").on_click(cx.listener(|v, _, _, cx| v.find_step(true, cx))))
                    .child(button("find-close", "close").on_click(cx.listener(|v, _, w, cx| v.close_find(w, cx)))),
            )
            .when(find.show_replace, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(div().flex_1().child(find.replace.clone()))
                        .child(
                            div().id("replace-one").px_2().rounded(px(4.)).cursor_pointer().bg(t.bg("button.secondaryBackground")).child("Replace")
                                .on_click(cx.listener(|v, _, _, cx| v.replace_one(cx))),
                        )
                        .child(
                            div().id("replace-all").px_2().rounded(px(4.)).cursor_pointer().bg(t.bg("button.secondaryBackground")).child("All")
                                .on_click(cx.listener(|v, _, _, cx| v.replace_all(cx))),
                        ),
                )
            })
    }
}
