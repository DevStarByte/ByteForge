//! Inserting snippets and moving through their fields: Tab selects the next field (`$1`, `$2`, …, then the
//! final caret `$0`), Shift+Tab the previous one, Escape leaves. Leaving a field copies its text to its
//! mirrors (e.g. the loop variable of `for`).

use bf_core::snippets;
use bf_lsp::{CompletionItem, CompletionKind};
use bf_text::{Change, Point};
use gpui::Context;

use super::EditorView;
use super::inlays::shift_point;

/// One inserted snippet's fields: each field's occurrences (the first is edited, the rest mirror it); the
/// last field is `$0`.
type Fields = Vec<Vec<(Point, Point)>>;

/// A snippet being filled in: one copy per cursor it was inserted at, all filled in together.
pub(crate) struct Session {
    copies: Vec<Fields>,
    /// The copy at the primary cursor.
    primary: usize,
    current: usize,
}

impl Session {
    /// Keeps the fields on their text while you type.
    pub fn shift(&mut self, c: &Change) {
        let current = self.current;
        for fields in &mut self.copies {
            for (i, field) in fields.iter_mut().enumerate() {
                for (s, e) in field {
                    // Typing at the start of the current field (e.g. into an empty one) goes into it.
                    let start = if i == current && *s == c.start && c.start == c.old_end { *s } else { shift_point(*s, c) };
                    let end = shift_point(*e, c);
                    *s = start;
                    *e = end.max(start);
                }
            }
        }
    }

    fn field_count(&self) -> usize {
        self.copies.first().map_or(0, Vec::len)
    }
}

/// The point `offset` chars after `start` in `text` (which is inserted at `start`).
fn point_after(start: Point, text: &str, offset: usize) -> Point {
    let mut p = start;
    for c in text.chars().take(offset) {
        if c == '\n' {
            p = Point::new(p.line + 1, 0);
        } else {
            p.col += 1;
        }
    }
    p
}

impl EditorView {
    /// Snippets as completion items (label = prefix).
    pub(crate) fn snippet_items(&self) -> Vec<CompletionItem> {
        self.snippets
            .iter()
            .map(|s| CompletionItem {
                label: s.prefix.clone(),
                insert_text: s.body.clone(),
                filter_text: s.prefix.clone(),
                // After the server's items with the same prefix.
                sort_text: format!("~{}", s.prefix),
                kind: CompletionKind::Snippet,
                detail: Some(s.description.clone()),
                text_edit_range: None,
                snippet: true,
            })
            .collect()
    }

    /// Replaces `start..end` (the word typed at the primary cursor) with the expanded `body` and selects its
    /// first field. With several cursors, every cursor that typed the same word gets its own copy, and the
    /// fields of all copies are filled in together.
    pub(crate) fn insert_snippet(&mut self, start: Point, end: Point, body: &str, cx: &mut Context<Self>) {
        let typed = self.state.buffer.slice(start, end);
        let width = end.col.saturating_sub(start.col);
        let primary_caret = self.state.caret();
        let mut carets: Vec<Point> = if self.state.has_multiple_cursors() && start.line == end.line {
            self.state.cursors().iter().map(|(c, _)| c.caret).filter(|c| c.col >= width && self.state.buffer.slice(Point::new(c.line, c.col - width), *c) == typed).collect()
        } else {
            vec![end]
        };
        if !carets.contains(&end) {
            carets.push(end);
        }
        // Bottom up, so inserting doesn't move the carets still to do; the session moves the copies done.
        carets.sort_by(|a, b| b.cmp(a));
        let tab = self.state.indent_unit();
        let path = self.path.clone();
        self.snippet_session = Some(Session { copies: Vec::new(), primary: 0, current: 0 });
        self.state.buffer.undo_stack().begin_group();
        let mut primary = 0;
        for caret in carets {
            let from = Point::new(caret.line, caret.col - width);
            let line = self.state.buffer.line(from.line);
            let indent: String = line.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
            let expansion = snippets::expand(body, &indent, &tab, &|name| snippets::variable(name, path.as_deref()));
            self.state.buffer.replace(from, caret, &expansion.text);
            self.after_edit(cx);
            let fields: Fields = expansion
                .stops
                .iter()
                .map(|occurrences| occurrences.iter().map(|r| (point_after(from, &expansion.text, r.start), point_after(from, &expansion.text, r.end))).collect())
                .collect();
            if let Some(session) = &mut self.snippet_session {
                if caret == primary_caret || caret == end {
                    primary = session.copies.len();
                }
                session.copies.push(fields);
            }
        }
        self.state.buffer.undo_stack().end_group();
        let Some(session) = &mut self.snippet_session else { return };
        session.primary = primary;
        if session.field_count() > 1 {
            self.select_snippet_field(cx);
        } else {
            // No fields: just the final caret(s).
            let carets: Vec<(Point, Point)> = session.copies.iter().filter_map(|f| f.first().and_then(|o| o.first()).map(|&(c, _)| (c, c))).collect();
            let primary = session.primary;
            self.snippet_session = None;
            self.state.set_selections(&carets, primary);
            self.after_move(cx);
        }
    }

    /// Selects the current field in every copy (one cursor each).
    fn select_snippet_field(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &self.snippet_session else { return };
        let selections: Vec<(Point, Point)> = session.copies.iter().filter_map(|f| f.get(session.current).and_then(|o| o.first()).copied()).collect();
        if selections.is_empty() {
            return;
        }
        let primary = session.primary.min(selections.len() - 1);
        let last = session.current + 1 == session.field_count();
        if last {
            // `$0` is where the caret ends up; the snippet is done.
            let carets: Vec<(Point, Point)> = selections.iter().map(|&(_, e)| (e, e)).collect();
            self.state.set_selections(&carets, primary);
            self.snippet_session = None;
        } else {
            self.state.set_selections(&selections, primary);
        }
        self.after_move(cx);
    }

    /// Tab (`delta` 1) / Shift+Tab (-1) inside a snippet; `false` when no snippet is being filled in.
    pub(crate) fn next_snippet_field(&mut self, delta: isize, cx: &mut Context<Self>) -> bool {
        let Some(session) = &self.snippet_session else { return false };
        let current = session.current;
        let target = (current as isize + delta).clamp(0, session.field_count() as isize - 1) as usize;
        self.update_mirrors(current, cx);
        if let Some(session) = &mut self.snippet_session {
            session.current = target;
        }
        self.select_snippet_field(cx);
        true
    }

    /// Copies field `index`'s text to its other occurrences, in every copy.
    fn update_mirrors(&mut self, index: usize, cx: &mut Context<Self>) {
        let copies = self.snippet_session.as_ref().map_or(0, |s| s.copies.len());
        for copy in 0..copies {
            let field = |v: &Self| v.snippet_session.as_ref().and_then(|s| s.copies.get(copy)).and_then(|f| f.get(index)).cloned();
            let mirrors = field(self).map_or(0, |f| f.len().saturating_sub(1));
            for m in 1..=mirrors {
                // Each edit shifts the session's positions (after_edit), so read them again every time.
                let Some(occurrences) = field(self) else { return };
                let ((s, e), (ms, me)) = (occurrences[0], occurrences[m]);
                let text = self.state.buffer.slice(s, e);
                if self.state.buffer.slice(ms, me) != text {
                    self.state.buffer.replace(ms, me, &text);
                    self.after_edit(cx);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_to_points() {
        let text = "if (x)\n    {\n    }";
        let start = Point::new(3, 4);
        assert_eq!(point_after(start, text, 0), start);
        assert_eq!(point_after(start, text, 5), Point::new(3, 9));
        assert_eq!(point_after(start, text, 7), Point::new(4, 0));
        assert_eq!(point_after(start, text, 11), Point::new(4, 4));
    }
}

#[cfg(test)]
mod view_tests {
    use super::*;
    use gpui::AppContext as _;

    /// A plain-text editor view (no language, no server) in a test app with the default theme.
    fn editor(cx: &mut gpui::TestAppContext, text: &str) -> gpui::Entity<EditorView> {
        cx.update(|cx| {
            let theme = crate::theme::Theme::build(std::sync::Arc::new(bf_core::theme::ThemeData::default()), 0, cx);
            theme.install(cx);
        });
        cx.new(|cx| EditorView::new(text, None, cx))
    }

    #[gpui::test]
    fn a_snippet_goes_to_every_cursor_and_its_fields_fill_together(cx: &mut gpui::TestAppContext) {
        let view = editor(cx, "    fo\n    fo\n");
        view.update(cx, |v, cx| {
            v.state.set_caret(Point::new(0, 6), false);
            v.state.toggle_cursor(Point::new(1, 6));
            // `for` with a loop variable mirrored three times.
            v.insert_snippet(Point::new(0, 4), Point::new(0, 6), "for (int ${1:i} = 0; $1 < ${2:n}; $1++)\n{\n\t$0\n}", cx);
            assert_eq!(v.state.cursor_count(), 2);
            v.edit(cx, |s| s.type_text("k"));
            assert!(v.next_snippet_field(1, cx));
            let text = v.state.buffer.text();
            assert_eq!(text.matches("for (int k = 0; k < n; k++)").count(), 2, "{text}");
            // Second field selected in both copies.
            assert!(v.state.cursors().iter().all(|(c, _)| v.state.buffer.slice(c.selection().0, c.selection().1) == "n"), "{text}");
            v.edit(cx, |s| s.type_text("count"));
            assert!(v.next_snippet_field(1, cx));
            assert_eq!(v.state.buffer.text().matches("k < count").count(), 2);
            // `$0`: one caret inside each body, the snippet done.
            assert_eq!(v.state.cursor_count(), 2);
            assert!(!v.next_snippet_field(1, cx));
        });
    }

    #[gpui::test]
    fn a_snippet_with_one_cursor_still_works(cx: &mut gpui::TestAppContext) {
        let view = editor(cx, "prop");
        view.update(cx, |v, cx| {
            v.state.set_caret(Point::new(0, 4), false);
            v.insert_snippet(Point::new(0, 0), Point::new(0, 4), "public ${1:int} ${2:Name} { get; set; }$0", cx);
            assert_eq!(v.state.selected_text(), "int");
            v.edit(cx, |s| s.type_text("string"));
            v.next_snippet_field(1, cx);
            assert_eq!(v.state.selected_text(), "Name");
            v.next_snippet_field(1, cx);
            assert_eq!(v.state.buffer.text(), "public string Name { get; set; }");
            assert_eq!(v.state.caret(), Point::new(0, 32));
        });
    }
}
