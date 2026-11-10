//! Completion: triggering, filtering, committing and the completion popup.

use super::*;

impl EditorView {
    pub(super) fn maybe_trigger_completion(&mut self, typed: &str, cx: &mut Context<Self>) {
        // Completions replace one range: not offered while editing with several cursors.
        if (self.lsp.is_none() && self.snippets.is_empty()) || self.state.has_multiple_cursors() {
            return;
        }
        let Some(c) = typed.chars().last() else { return };
        if let Some(completion) = &self.completion
            && (c.is_alphanumeric() || c == '_')
        {
            let _ = completion;
            self.refilter_completion();
            return;
        }
        let caret = self.state.caret();
        let line = self.state.buffer.line(caret.line);
        let before: Vec<char> = line.chars().take(caret.col).collect();
        let word_start = before.len() >= 2 && (before[before.len() - 2].is_alphanumeric() || before[before.len() - 2] == '_');
        if c == '.' || ((c.is_alphabetic() || c == '_') && !word_start) {
            self.request_completion(cx);
        } else if !(c.is_alphanumeric() || c == '_') {
            self.completion = None;
        }
    }

    pub(super) fn request_completion(&mut self, cx: &mut Context<Self>) {
        let snippets = self.snippet_items();
        let (Some(lsp), Some(path)) = (self.lsp.clone(), self.path.clone()) else {
            // No language server: snippets only, after a word start.
            if !snippets.is_empty() {
                let start = self.word_start();
                if start != self.state.caret() {
                    self.completion = Some(Completion { start, items: Arc::new(snippets), filtered: Vec::new(), selected: 0 });
                    self.refilter_completion();
                    cx.notify();
                }
            }
            return;
        };
        let caret = self.state.caret();
        let pos = self.lsp_position(caret);
        let version = self.state.buffer.version();
        // After a `.` only members make sense.
        let after_dot = caret.col > 0 && self.state.buffer.line(caret.line).chars().nth(caret.col - 1) == Some('.');
        self.completion_task = Some(cx.spawn(async move |this, cx| {
            let rx = lsp.completion(&path, pos);
            let Ok(Ok(mut items)) = rx.recv_async().await else { return };
            if !after_dot {
                items.extend(snippets);
            }
            if items.is_empty() {
                return;
            }
            let _ = this.update(cx, |view, cx| {
                let caret = view.state.caret();
                // Word start: from the server's edit range, else scan back over identifier chars.
                let start = items
                    .iter()
                    .find_map(|i| i.text_edit_range)
                    .map(|r| view.state.buffer.utf16_to_point(r.start.line as usize, r.start.character as usize))
                    .filter(|p| p.line == caret.line && p.col <= caret.col)
                    .unwrap_or_else(|| view.word_start());
                let _ = version;
                view.completion = Some(Completion { start, items: Arc::new(items), filtered: Vec::new(), selected: 0 });
                view.refilter_completion();
                cx.notify();
            });
        }));
    }

    pub(super) fn refilter_completion(&mut self) {
        let Some(completion) = &mut self.completion else { return };
        let caret = self.state.caret();
        let prefix = self.state.buffer.slice(completion.start, caret);
        let lower = prefix.to_lowercase();
        let mut scored: Vec<(usize, i32)> = completion
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| {
                let f = &item.filter_text;
                let score = if prefix.is_empty() {
                    0
                } else if f.starts_with(&prefix) {
                    3
                } else if f.to_lowercase().starts_with(&lower) {
                    2
                } else if subsequence(&f.to_lowercase(), &lower) {
                    1
                } else {
                    return None;
                };
                Some((i, score))
            })
            .collect();
        let items = &completion.items;
        scored.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then(items[a.0].filter_text.len().cmp(&items[b.0].filter_text.len()))
                .then(items[a.0].sort_text.cmp(&items[b.0].sort_text))
        });
        completion.filtered = scored.into_iter().take(500).map(|(i, _)| i).collect();
        completion.selected = 0;
        if completion.filtered.is_empty() {
            self.completion = None;
        }
    }

    pub(super) fn commit_completion(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(completion) = self.completion.take() else { return false };
        let Some(&index) = completion.filtered.get(completion.selected) else { return false };
        let item = &completion.items[index];
        let caret = self.state.caret();
        if item.snippet {
            self.insert_snippet(completion.start, caret, &item.insert_text, cx);
            return true;
        }
        self.state.replace_range(completion.start, caret, &item.insert_text);
        self.after_edit(cx);
        true
    }

    /// Where the identifier before the caret starts.
    pub(super) fn word_start(&self) -> Point {
        let caret = self.state.caret();
        let line: Vec<char> = self.state.buffer.line(caret.line).chars().collect();
        let mut s = caret.col.min(line.len());
        while s > 0 && (line[s - 1].is_alphanumeric() || line[s - 1] == '_') {
            s -= 1;
        }
        Point::new(caret.line, s)
    }

    // ───────────── find ─────────────
}

impl EditorView {
    pub(super) fn completion_nav(&mut self, delta: isize, cx: &mut Context<Self>) -> bool {
        let Some(c) = &mut self.completion else { return false };
        let n = c.filtered.len() as isize;
        if n == 0 {
            return false;
        }
        c.selected = (c.selected as isize + delta).clamp(0, n - 1) as usize;
        cx.notify();
        true
    }
}

impl EditorView {
    pub(super) fn render_completion(&self, _: &mut Window, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let completion = self.completion.as_ref()?;
        let t = Theme::global(cx);
        let (x, y) = self.caret_screen_pos(completion.start);
        let lh = self.layout.line_height;
        let row_h = t.n("list.itemHeight", 22.0).max(lh * 0.9);
        let max_rows = (t.n("completion.maxHeight", 300.0) / row_h).floor().max(3.0) as usize;
        let selected = completion.selected;
        let first = selected.saturating_sub(max_rows - 1).min(completion.filtered.len().saturating_sub(max_rows));
        let rows: Vec<_> = completion
            .filtered
            .iter()
            .enumerate()
            .skip(first)
            .take(max_rows)
            .map(|(i, &index)| {
                let item = &completion.items[index];
                let (glyph, token) = kind_glyph(item.kind);
                let color = t.token(token).map(|s| s.color).unwrap_or(t.c("editor.foreground"));
                div()
                    .h(px(row_h))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded(t.radius("list.radius", 0.0))
                    .when(i == selected, |d| d.bg(t.bg("list.activeSelectionBackground")).text_color(t.c("list.activeSelectionForeground")))
                    .child(div().w(px(16.)).text_color(color).font_weight(gpui::FontWeight::BOLD).child(glyph))
                    .child(div().flex_1().overflow_hidden().whitespace_nowrap().child(item.label.clone()))
                    .when_some(item.detail.clone(), |d, detail| {
                        d.child(div().text_color(t.c("foreground.muted")).overflow_hidden().whitespace_nowrap().max_w(px(180.)).child(detail))
                    })
            })
            .collect();
        Some(
            div()
                .absolute()
                .left(px(x))
                .top(px(y + lh))
                .w(t.m("completion.width", 440.0))
                .bg(t.bg("editorWidget.background"))
                .border_color(t.c("editorWidget.border"))
                .border_1()
                .rounded(t.radius("widget.radius", 6.0))
                .shadow(t.shadow("widget.shadow"))
                .p(t.edges("completion.padding", 2.0).top)
                .text_size(t.m("ui.size", 13.0))
                .font_family(t.editor_font.clone())
                .text_color(t.c("editor.foreground"))
                .children(rows),
        )
    }
}

pub(super) fn subsequence(text: &str, query: &str) -> bool {
    let mut q = query.chars().peekable();
    let first = query.chars().next();
    if text.chars().next() != first {
        return false;
    }
    for c in text.chars() {
        if q.peek() == Some(&c) {
            q.next();
        }
    }
    q.peek().is_none()
}

pub(super) fn kind_glyph(kind: CompletionKind) -> (&'static str, &'static str) {
    use CompletionKind::*;
    match kind {
        Method | Function | Constructor => ("M", "method name"),
        Class => ("C", "class name"),
        Struct => ("S", "struct name"),
        Interface => ("I", "interface name"),
        Enum => ("E", "enum name"),
        EnumMember => ("e", "enum member name"),
        Property => ("P", "property name"),
        Field => ("F", "field name"),
        Event => ("V", "event name"),
        Constant => ("K", "constant name"),
        Variable => ("L", "local name"),
        Module => ("N", "namespace name"),
        TypeParameter => ("T", "type parameter name"),
        Keyword => ("k", "keyword"),
        Snippet => ("s", "keyword"),
        _ => ("·", "identifier"),
    }
}
