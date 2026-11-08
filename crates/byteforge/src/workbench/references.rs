//! The References panel (Find All References, implementations, type definitions) and applying Rename Symbol across files.

use super::*;

/// One Find All References result.
pub(super) struct References {
    /// What was looked for, e.g. "references to 'Count'".
    pub title: String,
    pub items: Vec<ReferenceItem>,
}

pub(super) struct ReferenceItem {
    pub path: PathBuf,
    pub at: Point,
    /// The line's text (leading whitespace trimmed) and the reference's char range in it.
    pub line_text: String,
    pub highlight: std::ops::Range<usize>,
}

impl Workbench {
    /// Fills the References panel: line text from open editors (unsaved changes included), else from disk.
    pub(super) fn show_references(&mut self, title: String, locations: &[bf_lsp::Location], cx: &mut Context<Self>) {
        let mut files: HashMap<PathBuf, Option<bf_text::Buffer>> = HashMap::new();
        let mut items = Vec::new();
        for loc in locations {
            let path = normalize_path(&loc.path);
            let open = self.tabs.iter().find(|t| t.path.as_deref().is_some_and(|p| bf_core::paths::same_path(p, &path)));
            let (at, line) = match open {
                Some(tab) => {
                    let buffer = &tab.editor.read(cx).state.buffer;
                    let at = buffer.utf16_to_point(loc.range.start.line as usize, loc.range.start.character as usize);
                    let end = buffer.utf16_to_point(loc.range.end.line as usize, loc.range.end.character as usize);
                    (at..end, buffer.line(at.line).to_string())
                }
                None => {
                    let buffer = files.entry(path.clone()).or_insert_with(|| std::fs::read_to_string(&path).ok().map(|t| bf_text::Buffer::new(&t)));
                    match buffer {
                        Some(buffer) => {
                            let at = buffer.utf16_to_point(loc.range.start.line as usize, loc.range.start.character as usize);
                            let end = buffer.utf16_to_point(loc.range.end.line as usize, loc.range.end.character as usize);
                            (at..end, buffer.line(at.line).to_string())
                        }
                        None => {
                            let at = Point::new(loc.range.start.line as usize, loc.range.start.character as usize);
                            (at..at, String::new())
                        }
                    }
                }
            };
            let line = line.trim_end_matches(['\n', '\r']);
            let indent = line.chars().take_while(|c| c.is_whitespace()).count();
            let end_col = if at.end.line == at.start.line { at.end.col } else { line.chars().count() };
            items.push(ReferenceItem {
                path,
                at: at.start,
                line_text: line.chars().skip(indent).collect(),
                highlight: at.start.col.saturating_sub(indent)..end_col.saturating_sub(indent),
            });
        }
        items.sort_by(|a, b| (&a.path, a.at).cmp(&(&b.path, b.at)));
        items.dedup_by(|a, b| a.path == b.path && a.at == b.at);
        let count = items.len();
        let file_count = items.iter().map(|i| &i.path).collect::<std::collections::HashSet<_>>().len();
        if count == 0 {
            self.log(format!("No {title} found."));
            self.show_panel(PanelView::Output, cx);
            return;
        }
        self.log(format!("{count} {title} in {file_count} file(s)."));
        self.references = Some(References { title, items });
        self.show_panel(PanelView::References, cx);
    }

    /// Applies a rename: open files change in their editors (undoable), closed files on disk.
    pub(super) fn apply_rename(&mut self, edit: &bf_lsp::WorkspaceEdit, _window: &mut Window, cx: &mut Context<Self>) {
        let places: usize = edit.changes.iter().map(|(_, e)| e.len()).sum();
        let files = edit.changes.iter().filter(|(_, e)| !e.is_empty()).count();
        self.apply_workspace_edit(edit, None, cx);
        self.log(format!("Renamed {places} occurrence(s) in {files} file(s)."));
    }

    pub(super) fn render_references(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = Theme::global(cx);
        let muted = t.c("foreground.muted");
        let Some(refs) = &self.references else {
            return div().p_3().text_color(muted).child("Use Find All References (Shift+F12) on a symbol.").into_any_element();
        };
        let root = self.workspace.root();
        let highlight_bg = t.c_or("editor.findMatchHighlightBackground", 0xea5c0055);
        let mut rows: Vec<gpui::AnyElement> = Vec::new();
        rows.push(
            div()
                .px_3()
                .py(px(2.))
                .text_color(muted)
                .child(format!("{} {}", refs.items.len(), refs.title))
                .into_any_element(),
        );
        let mut current: Option<&PathBuf> = None;
        for (i, item) in refs.items.iter().enumerate() {
            if current != Some(&item.path) {
                current = Some(&item.path);
                let count = refs.items.iter().filter(|r| r.path == item.path).count();
                let name = item.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let dir = item.path.parent().map(|p| root.as_ref().and_then(|r| p.strip_prefix(r).ok()).unwrap_or(p).display().to_string()).unwrap_or_default();
                rows.push(
                    div()
                        .px_3()
                        .pt_1()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child({
                                    let (icon_name, color_key) = crate::explorer::file_icon(&name, false, false);
                                    icon(icon_name, t.m("explorer.iconSize", 15.0), crate::explorer::file_icon_color(&t, color_key))
                                })
                        .child(div().whitespace_nowrap().font_weight(gpui::FontWeight::SEMIBOLD).child(name))
                        .child(div().whitespace_nowrap().text_size(px(11.)).text_color(muted).child(dir))
                        .child(div().px(px(6.)).rounded(px(9.)).text_size(px(11.)).bg(t.bg("badge.background")).text_color(t.c("badge.foreground")).child(count.to_string()))
                        .into_any_element(),
                );
            }
            let chars: Vec<char> = item.line_text.chars().collect();
            let (s, e) = (item.highlight.start.min(chars.len()), item.highlight.end.min(chars.len()));
            let (before, matched, after): (String, String, String) =
                (chars[..s].iter().collect(), chars[s..e.max(s)].iter().collect(), chars[e.max(s)..].iter().collect());
            let (path, at) = (item.path.clone(), item.at);
            rows.push(
                div()
                    .id(SharedString::from(format!("ref-{i}")))
                    .pl(px(40.))
                    .pr_3()
                    .py(px(1.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .hover(|s| s.bg(t.c("list.hoverBackground")))
                    .child(div().w(px(44.)).flex_none().text_size(px(11.)).text_color(muted).child(format!("{}", item.at.line + 1)))
                    .child(
                        div()
                            .flex()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .font_family(t.editor_font.clone())
                            .child(before)
                            .child(div().bg(highlight_bg).rounded(px(2.)).child(matched))
                            .child(after),
                    )
                    .on_click(cx.listener(move |wb, _, w, cx| wb.open_file(path.clone(), Some(at), w, cx)))
                    .into_any_element(),
            );
        }
        div().id("references").size_full().overflow_y_scroll().text_size(px(12.)).children(rows).into_any_element()
    }
}
