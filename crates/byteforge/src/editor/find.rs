//! Find and replace, and the find bar: match case, whole word and regular expressions (with `$1` in the
//! replacement).

use super::*;

impl FindState {
    /// The query as the toggles say (`Ok(None)` while it is empty).
    pub(super) fn matcher(&self, cx: &App) -> Result<Option<bf_text::Matcher>, String> {
        bf_text::Matcher::new(&bf_text::Query {
            text: self.query.read(cx).text().to_owned(),
            match_case: self.match_case,
            whole_word: self.whole_word,
            regex: self.regex,
        })
    }
}

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
            self.find = Some(FindState {
                query,
                replace: replace_input,
                show_replace: replace,
                match_case: false,
                whole_word: false,
                regex: false,
                error: None,
                matches: Vec::new(),
                _subs: subs,
            });
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
        let (matches, error) = match find.matcher(cx) {
            Ok(Some(m)) => (self.state.find_matches(&m, 50_000), None),
            Ok(None) => (Vec::new(), None),
            Err(e) => (Vec::new(), Some(e)),
        };
        if select && !matches.is_empty() {
            let (start, _) = self.state.selection();
            let m = matches.iter().find(|m| m.0 >= start).copied().unwrap_or(matches[0]);
            self.state.select(m.0, m.1);
            self.reveal(m.0, false);
        }
        if let Some(find) = &mut self.find {
            find.matches = matches;
            find.error = error;
        }
        cx.notify();
    }

    /// The text that replaces the match `m`: the replace field, with `$1` … filled in for a regex.
    fn replacement_for(&self, m: (Point, Point), template: &str, matcher: Option<&bf_text::Matcher>) -> String {
        match matcher {
            Some(matcher) if self.find.as_ref().is_some_and(|f| f.regex) => {
                let line = self.state.buffer.line(m.0.line);
                let byte = line.char_indices().nth(m.0.col).map_or(line.len(), |(b, _)| b);
                matcher.replacement(&line, byte, template)
            }
            _ => template.to_owned(),
        }
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
        let template = find.replace.read(cx).text().to_owned();
        let matcher = find.matcher(cx).ok().flatten();
        let sel = self.state.selection();
        if find.matches.contains(&sel) {
            let replacement = self.replacement_for(sel, &template, matcher.as_ref());
            self.state.insert(&replacement);
            self.after_edit(cx);
        }
        self.find_step(true, cx);
    }

    pub(super) fn replace_all(&mut self, cx: &mut Context<Self>) {
        let Some(find) = &self.find else { return };
        let template = find.replace.read(cx).text().to_owned();
        let matcher = find.matcher(cx).ok().flatten();
        let matches = find.matches.clone();
        // Worked out on the text as it is, then applied from the end so earlier positions stay valid.
        let replacements: Vec<String> = matches.iter().map(|m| self.replacement_for(*m, &template, matcher.as_ref())).collect();
        self.state.buffer.undo_stack().begin_group();
        for ((s, e), text) in matches.iter().zip(&replacements).rev() {
            self.state.buffer.replace(*s, *e, text);
        }
        self.state.buffer.undo_stack().end_group();
        self.after_edit(cx);
        self.update_find(false, cx);
    }

    /// Flips a find option (the toggle buttons, Alt+C / Alt+W / Alt+R) and finds again.
    pub(super) fn toggle_find_option(&mut self, option: FindOption, cx: &mut Context<Self>) {
        let Some(f) = &mut self.find else { return };
        match option {
            FindOption::Case => f.match_case = !f.match_case,
            FindOption::Word => f.whole_word = !f.whole_word,
            FindOption::Regex => f.regex = !f.regex,
        }
        self.update_find(true, cx);
    }

    /// Alt+C / Alt+W / Alt+R: only while the find bar is open (otherwise the key goes on as usual).
    pub(super) fn find_option_key(&mut self, option: FindOption, cx: &mut Context<Self>) {
        if self.find.is_some() {
            self.toggle_find_option(option, cx);
        } else {
            cx.propagate();
        }
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
            _ if find.error.is_some() => "Invalid".into(),
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
        let toggle = |id: &'static str, label: &'static str, on: bool, tip: &'static str| {
            div()
                .id(id)
                .px_1()
                .rounded(px(3.))
                .cursor_pointer()
                .when(on, |d| d.bg(t.bg("button.background")).text_color(t.c("button.foreground")))
                .when(!on, |d| d.hover(move |s| s.bg(hover_bg)))
                .child(label)
                .tooltip(crate::workbench::tooltip(tip))
        };
        let error = find.error.clone();
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
                    .child(toggle("match-case", "Aa", find.match_case, if cfg!(target_os = "macos") { "Match Case (⌘⌥C)" } else { "Match Case (Alt+C)" }).on_click(cx.listener(|v, _, _, cx| v.toggle_find_option(FindOption::Case, cx))))
                    .child(toggle("whole-word", "ab", find.whole_word, if cfg!(target_os = "macos") { "Match Whole Word (⌘⌥W)" } else { "Match Whole Word (Alt+W)" }).underline().on_click(cx.listener(|v, _, _, cx| v.toggle_find_option(FindOption::Word, cx))))
                    .child(toggle("use-regex", ".*", find.regex, if cfg!(target_os = "macos") { "Use Regular Expression (⌘⌥R); $1 in Replace inserts a group" } else { "Use Regular Expression (Alt+R); $1 in Replace inserts a group" }).on_click(cx.listener(|v, _, _, cx| v.toggle_find_option(FindOption::Regex, cx))))
                    .child(div().min_w(px(70.)).text_color(if error.is_some() { t.c("errorForeground") } else { t.c("foreground.muted") }).child(label))
                    .child(button("find-prev", "chevronUp").on_click(cx.listener(|v, _, _, cx| v.find_step(false, cx))))
                    .child(button("find-next", "chevronDown").on_click(cx.listener(|v, _, _, cx| v.find_step(true, cx))))
                    .child(button("find-close", "close").on_click(cx.listener(|v, _, w, cx| v.close_find(w, cx)))),
            )
            .when_some(error, |d, e| d.child(div().text_size(px(11.)).text_color(t.c("errorForeground")).child(e)))
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

#[derive(Clone, Copy)]
pub(super) enum FindOption {
    Case,
    Word,
    Regex,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor_window<'a>(cx: &'a mut gpui::TestAppContext, text: &str) -> (Entity<EditorView>, &'a mut gpui::VisualTestContext) {
        cx.update(|cx| crate::theme::Theme::build(std::sync::Arc::new(bf_core::theme::ThemeData::default()), 0, cx).install(cx));
        cx.add_window_view(|_, cx| EditorView::new(text, None, cx))
    }

    #[gpui::test]
    fn regex_find_and_replace_with_groups(cx: &mut gpui::TestAppContext) {
        let (view, cx) = editor_window(cx, "var a = 1;\nvar total = 22;\nvar counter = 3;\n");
        view.update_in(cx, |v, window, cx| {
            v.open_find(true, window, cx);
            let find = v.find.as_ref().unwrap();
            let (query, replace) = (find.query.clone(), find.replace.clone());
            query.update(cx, |q, cx| q.set_text(r"var (\w+) = (\d+)".to_owned(), cx));
            replace.update(cx, |r, cx| r.set_text("let $1: i32 = $2".to_owned(), cx));
            v.update_find(false, cx);
            assert_eq!(v.find_matches().len(), 0, "a regex isn't used until .* is on");
            v.toggle_find_option(FindOption::Regex, cx);
            assert_eq!(v.find_matches().len(), 3);
            v.replace_all(cx);
            assert_eq!(v.state.buffer.text(), "let a: i32 = 1;\nlet total: i32 = 22;\nlet counter: i32 = 3;\n");
        });
    }

    #[gpui::test]
    fn whole_word_and_invalid_regex(cx: &mut gpui::TestAppContext) {
        let (view, cx) = editor_window(cx, "count counter count_ count\n");
        view.update_in(cx, |v, window, cx| {
            v.open_find(false, window, cx);
            let query = v.find.as_ref().unwrap().query.clone();
            query.update(cx, |q, cx| q.set_text("count".to_owned(), cx));
            v.update_find(false, cx);
            assert_eq!(v.find_matches().len(), 4);
            v.toggle_find_option(FindOption::Word, cx);
            assert_eq!(v.find_matches(), [(Point::new(0, 0), Point::new(0, 5)), (Point::new(0, 21), Point::new(0, 26))]);
            v.toggle_find_option(FindOption::Regex, cx);
            query.update(cx, |q, cx| q.set_text("count(".to_owned(), cx));
            v.update_find(false, cx);
            assert!(v.find_matches().is_empty());
            assert!(v.find.as_ref().unwrap().error.is_some());
        });
    }
}
