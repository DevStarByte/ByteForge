//! Inserting snippets and moving through their fields: Tab selects the next field (`$1`, `$2`, …, then the
//! final caret `$0`), Shift+Tab the previous one, Escape leaves. Leaving a field copies its text to its
//! mirrors (e.g. the loop variable of `for`).

use bf_core::snippets;
use bf_lsp::{CompletionItem, CompletionKind};
use bf_text::{Change, Point};
use gpui::Context;

use super::EditorView;
use super::inlays::shift_point;

/// A snippet being filled in.
pub(crate) struct Session {
    /// Each field's occurrences (the first is edited, the rest mirror it); the last field is `$0`.
    fields: Vec<Vec<(Point, Point)>>,
    current: usize,
}

impl Session {
    /// Keeps the fields on their text while you type.
    pub fn shift(&mut self, c: &Change) {
        let current = self.current;
        for (i, field) in self.fields.iter_mut().enumerate() {
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

    /// Replaces `start..end` with the expanded `body` and selects its first field.
    pub(crate) fn insert_snippet(&mut self, start: Point, end: Point, body: &str, cx: &mut Context<Self>) {
        let line = self.state.buffer.line(start.line);
        let indent: String = line.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        let tab = self.state.indent_unit();
        let path = self.path.clone();
        let expansion = snippets::expand(body, &indent, &tab, &|name| snippets::variable(name, path.as_deref()));
        self.state.replace_range(start, end, &expansion.text);
        let fields: Vec<Vec<(Point, Point)>> = expansion
            .stops
            .iter()
            .map(|occurrences| occurrences.iter().map(|r| (point_after(start, &expansion.text, r.start), point_after(start, &expansion.text, r.end))).collect())
            .collect();
        self.snippet_session = None;
        self.after_edit(cx);
        if fields.len() > 1 {
            self.snippet_session = Some(Session { fields, current: 0 });
            self.select_snippet_field(cx);
        } else if let Some(&(caret, _)) = fields.first().and_then(|f| f.first()) {
            self.state.set_caret(caret, false);
            self.after_move(cx);
        }
    }

    fn select_snippet_field(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &self.snippet_session else { return };
        let Some(&(s, e)) = session.fields.get(session.current).and_then(|f| f.first()) else { return };
        let last = session.current + 1 == session.fields.len();
        self.state.select(s, e);
        if last {
            // `$0` is where the caret ends up; the snippet is done.
            self.state.set_caret(e, false);
            self.snippet_session = None;
        }
        self.after_move(cx);
    }

    /// Tab (`delta` 1) / Shift+Tab (-1) inside a snippet; `false` when no snippet is being filled in.
    pub(crate) fn next_snippet_field(&mut self, delta: isize, cx: &mut Context<Self>) -> bool {
        let Some(session) = &self.snippet_session else { return false };
        let current = session.current;
        let target = (current as isize + delta).clamp(0, session.fields.len() as isize - 1) as usize;
        self.update_mirrors(current, cx);
        if let Some(session) = &mut self.snippet_session {
            session.current = target;
        }
        self.select_snippet_field(cx);
        true
    }

    /// Copies field `index`'s text to its other occurrences.
    fn update_mirrors(&mut self, index: usize, cx: &mut Context<Self>) {
        let mirrors = self.snippet_session.as_ref().and_then(|s| s.fields.get(index)).map_or(0, |f| f.len().saturating_sub(1));
        for m in 1..=mirrors {
            let Some(field) = self.snippet_session.as_ref().and_then(|s| s.fields.get(index)) else { return };
            let ((s, e), (ms, me)) = (field[0], field[m]);
            let text = self.state.buffer.slice(s, e);
            if self.state.buffer.slice(ms, me) != text {
                // Each edit shifts the session's positions (after_edit), so read them again every time.
                self.state.replace_range(ms, me, &text);
                self.after_edit(cx);
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
