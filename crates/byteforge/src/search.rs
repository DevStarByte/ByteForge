//! Search in files: streaming background search with a virtualized result list.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bf_core::Workspace;
use bf_core::search::{SearchHit, SearchMessage, search};
use bf_text::Point;
use gpui::{App, Context, Entity, EventEmitter, FocusHandle, Focusable, HighlightStyle, SharedString, StyledText, Subscription, Task, Window, div, prelude::*, px, uniform_list};

use crate::text_input::{InputEvent, TextInput};
use crate::theme::Theme;

pub enum SearchEvent {
    Open(PathBuf, Point),
    /// Replace the matches of `matcher` with `template`: in `files`, or (with `line`) on one line of one file.
    Replace { files: Vec<PathBuf>, line: Option<usize>, matcher: bf_text::Matcher, template: String, count: usize },
}

pub struct SearchView {
    workspace: Workspace,
    input: Entity<TextInput>,
    replace: Entity<TextInput>,
    show_replace: bool,
    /// What the current results were found with (Replace uses the same).
    matcher: Option<bf_text::Matcher>,
    match_case: bool,
    whole_word: bool,
    regex: bool,
    hits: Vec<SearchHit>,
    summary: String,
    cancel: Arc<AtomicBool>,
    task: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<SearchEvent> for SearchView {}

impl Focusable for SearchView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl SearchView {
    pub fn new(workspace: Workspace, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextInput::new("Search (Enter)", cx));
        let replace = cx.new(|cx| TextInput::new("Replace (Enter: Replace All)", cx));
        let subs = vec![
            cx.subscribe_in(&input, window, |this, _, event, _, cx| {
                if let InputEvent::Confirmed = event {
                    this.run(cx);
                }
            }),
            cx.subscribe_in(&replace, window, |this, _, event, _, cx| match event {
                InputEvent::Confirmed => this.replace_all(cx),
                // The preview in the results follows the replacement as it is typed.
                InputEvent::Changed => cx.notify(),
                _ => {}
            }),
        ];
        Self {
            workspace,
            input,
            replace,
            show_replace: false,
            matcher: None,
            match_case: false,
            whole_word: false,
            regex: false,
            hits: Vec::new(),
            summary: String::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            task: None,
            _subs: subs,
        }
    }

    /// Searches again (after a replace changed the files).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.run(cx);
    }

    fn template(&self, cx: &App) -> String {
        self.replace.read(cx).text().to_owned()
    }

    /// Replace All: the matches in every file of the results.
    fn replace_all(&mut self, cx: &mut Context<Self>) {
        let Some(matcher) = self.matcher.clone() else { return };
        let mut files: Vec<PathBuf> = Vec::new();
        for hit in &self.hits {
            if files.last() != Some(&hit.path) && !files.contains(&hit.path) {
                files.push(hit.path.clone());
            }
        }
        let count = self.hits.iter().map(|h| h.ranges.len()).sum();
        if count > 0 {
            cx.emit(SearchEvent::Replace { files, line: None, matcher, template: self.template(cx), count });
        }
    }

    fn replace_hit(&mut self, ix: usize, cx: &mut Context<Self>) {
        let (Some(matcher), Some(hit)) = (self.matcher.clone(), self.hits.get(ix)) else { return };
        cx.emit(SearchEvent::Replace { files: vec![hit.path.clone()], line: Some(hit.line), matcher, template: self.template(cx), count: hit.ranges.len() });
    }

    fn run(&mut self, cx: &mut Context<Self>) {
        self.cancel.store(true, Ordering::Relaxed);
        self.hits.clear();
        let query = self.input.read(cx).text().to_owned();
        if query.is_empty() || self.workspace.root().is_none() {
            self.summary = if self.workspace.root().is_none() { "Open a folder to search.".into() } else { String::new() };
            cx.notify();
            return;
        }
        let matcher = match bf_text::Matcher::new(&bf_text::Query { text: query, match_case: self.match_case, whole_word: self.whole_word, regex: self.regex }) {
            Ok(Some(m)) => m,
            Ok(None) => return,
            Err(e) => {
                self.summary = format!("Invalid regular expression: {e}");
                cx.notify();
                return;
            }
        };
        self.matcher = Some(matcher.clone());
        self.summary = "Searching…".into();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = cancel.clone();
        let workspace = self.workspace.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let files = cx.background_executor().spawn(async move { workspace.files() }).await;
            let rx = search(files, matcher, cancel);
            while let Ok(message) = rx.recv_async().await {
                let done = matches!(message, SearchMessage::Done { .. });
                let alive = this
                    .update(cx, |view, cx| {
                        match message {
                            SearchMessage::Hits(hits) => view.hits.extend(hits),
                            SearchMessage::Done { hits, files, millis, truncated } => {
                                view.summary = format!("{hits}{} results in {files} files ({millis} ms)", if truncated { "+" } else { "" });
                            }
                        }
                        cx.notify();
                    })
                    .is_ok();
                if done || !alive {
                    break;
                }
            }
        }));
        cx.notify();
    }
}

impl Render for SearchView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::global(cx);
        let entity = cx.entity();
        let root = self.workspace.root();
        let row_h = t.n("list.itemHeight", 22.0) * 2.0;
        let template = self.show_replace.then(|| self.template(cx));
        let list = uniform_list("search-results", self.hits.len(), move |range, _, cx| {
            let view = entity.read(cx);
            let t = Theme::global(cx);
            let hover = t.c("toolbar.hoverBackground");
            range
                .map(|ix| {
                    let hit = &view.hits[ix];
                    let text = match (&template, &view.matcher) {
                        (Some(template), Some(matcher)) => replace_preview(hit, matcher, template, &t),
                        _ => preview(hit, &t),
                    };
                    let replace_entity = entity.clone();
                    let rel = root.as_ref().and_then(|r| hit.path.strip_prefix(r).ok()).unwrap_or(&hit.path).display().to_string();
                    let (path, point) = (hit.path.clone(), Point::new(hit.line, hit.col));
                    let entity = entity.clone();
                    div()
                        .id(ix)
                        .h(px(row_h))
                        .px_3()
                        .flex()
                        .flex_col()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|s| s.bg(t.c("list.hoverBackground")))
                        .group("search-hit")
                        .relative()
                        .child(div().whitespace_nowrap().overflow_hidden().child(text))
                        .child(div().whitespace_nowrap().overflow_hidden().text_size(px(11.)).text_color(t.c("foreground.muted")).child(format!("{rel}:{}", hit.line + 1)))
                        .when(template.is_some(), |d| {
                            d.child(
                                div()
                                    .id(("replace-hit", ix))
                                    .absolute()
                                    .right(px(8.))
                                    .top(px(4.))
                                    .px_1()
                                    .rounded(px(3.))
                                    .invisible()
                                    .group_hover("search-hit", |s| s.visible())
                                    .bg(t.bg("editorWidget.background"))
                                    .hover(move |s| s.bg(hover))
                                    .text_size(px(11.))
                                    .child("Replace")
                                    .tooltip(crate::workbench::tooltip("Replace the matches on this line"))
                                    .on_click(move |_, _, cx| {
                                        cx.stop_propagation();
                                        replace_entity.update(cx, |v, cx| v.replace_hit(ix, cx));
                                    }),
                            )
                        })
                        .on_click(move |_, _, cx| entity.update(cx, |_, cx| cx.emit(SearchEvent::Open(path.clone(), point))))
                })
                .collect()
        })
        .flex_1();

        let show_replace = self.show_replace;
        let replace_row = div()
            .px_3()
            .pl(px(30.))
            .flex()
            .items_center()
            .gap_1()
            .child(div().flex_1().child(self.replace.clone()))
            .child(
                div()
                    .id("replace-all-files")
                    .px_2()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .bg(t.bg("button.secondaryBackground"))
                    .child("All")
                    .tooltip(crate::workbench::tooltip("Replace All (Enter in the Replace field)"))
                    .on_click(cx.listener(|v, _, _, cx| v.replace_all(cx))),
            );
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
                    .child(
                        div()
                            .id("toggle-replace")
                            .cursor_pointer()
                            .child(crate::assets::icon(if show_replace { "chevronDown" } else { "chevronRight" }, px(14.), t.c("icon.foreground")))
                            .tooltip(crate::workbench::tooltip("Toggle Replace"))
                            .on_click(cx.listener(|v, _, window, cx| {
                                v.show_replace = !v.show_replace;
                                if v.show_replace {
                                    window.focus(&v.replace.focus_handle(cx));
                                }
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1().child(self.input.clone()))
                    .child(toggle("search-case", "Aa", self.match_case, "Match Case", &t).on_click(cx.listener(|v, _, _, cx| {
                        v.match_case = !v.match_case;
                        v.run(cx);
                    })))
                    .child(toggle("search-word", "ab", self.whole_word, "Match Whole Word", &t).underline().on_click(cx.listener(|v, _, _, cx| {
                        v.whole_word = !v.whole_word;
                        v.run(cx);
                    })))
                    .child(toggle("search-regex", ".*", self.regex, "Use Regular Expression", &t).on_click(cx.listener(|v, _, _, cx| {
                        v.regex = !v.regex;
                        v.run(cx);
                    }))),
            )
            .when(show_replace, |d| d.child(replace_row))
            .child(div().px_3().text_size(px(12.)).text_color(t.c("foreground.muted")).child(self.summary.clone()))
            .child(list)
    }
}

/// An option button of the search box ("Aa", "ab", ".*").
fn toggle(id: &'static str, label: &'static str, on: bool, tip: &'static str, t: &Theme) -> gpui::Stateful<gpui::Div> {
    let hover = t.c("toolbar.hoverBackground");
    div()
        .id(id)
        .px_1()
        .rounded(px(3.))
        .cursor_pointer()
        .when(on, |d| d.bg(t.bg("button.background")).text_color(t.c("button.foreground")))
        .when(!on, |d| d.hover(move |s| s.bg(hover)))
        .child(label)
        .tooltip(crate::workbench::tooltip(tip))
}

/// The hit's line (trimmed, at most 200 chars) with its matches highlighted.
fn preview(hit: &SearchHit, t: &Theme) -> StyledText {
    let lead = hit.text.chars().take_while(|c| c.is_whitespace()).count();
    let shown: String = hit.text.chars().skip(lead).take(200).collect();
    let shown = shown.trim_end().to_owned();
    let len = shown.chars().count();
    // Char ranges of the full line → byte ranges of the shown text.
    let byte_at = |col: usize| shown.char_indices().nth(col).map_or(shown.len(), |(b, _)| b);
    let style = HighlightStyle { background_color: Some(t.c_or("editor.findMatchHighlightBackground", 0xea5c0055)), ..Default::default() };
    let highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = hit
        .ranges
        .iter()
        .filter_map(|&(s, e)| {
            let (s, e) = (s.saturating_sub(lead).min(len), e.saturating_sub(lead).min(len));
            (e > s).then(|| (byte_at(s)..byte_at(e), style))
        })
        .collect();
    StyledText::new(SharedString::from(shown)).with_highlights(highlights)
}

/// The hit's line (leading space trimmed) with each match followed by its replacement: the text, and the
/// byte ranges of the removed (`false`) and inserted (`true`) parts.
fn replace_preview_parts(hit: &SearchHit, matcher: &bf_text::Matcher, template: &str) -> (String, Vec<(std::ops::Range<usize>, bool)>) {
    let line = hit.text.as_str();
    let byte_of = |col: usize| line.char_indices().nth(col).map_or(line.len(), |(b, _)| b);
    let lead = line.len() - line.trim_start().len();
    let mut text = String::new();
    let mut parts = Vec::new();
    let mut last = lead;
    for &(s, e) in &hit.ranges {
        let (s, e) = (byte_of(s).max(lead), byte_of(e).max(lead));
        text.push_str(&line[last..s]);
        let at = text.len();
        text.push_str(&line[s..e]);
        parts.push((at..text.len(), false));
        let replacement = matcher.replacement(line, s, template);
        let at = text.len();
        text.push_str(&replacement);
        if !replacement.is_empty() {
            parts.push((at..text.len(), true));
        }
        last = e;
    }
    text.push_str(line[last..].trim_end());
    (text, parts)
}

/// The hit's line with each match struck out and its replacement after it.
fn replace_preview(hit: &SearchHit, matcher: &bf_text::Matcher, template: &str, t: &Theme) -> StyledText {
    let removed = HighlightStyle {
        background_color: Some(t.c_or("diffEditor.removedTextBackground", 0xf8514966)),
        strikethrough: Some(gpui::StrikethroughStyle { thickness: px(1.), color: None }),
        ..Default::default()
    };
    let added = HighlightStyle { background_color: Some(t.c_or("diffEditor.insertedTextBackground", 0x2ea04366)), ..Default::default() };
    let (text, parts) = replace_preview_parts(hit, matcher, template);
    let highlights: Vec<_> = parts.into_iter().map(|(range, inserted)| (range, if inserted { added } else { removed })).collect();
    StyledText::new(SharedString::from(text)).with_highlights(highlights)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn hit(path: &str, line: usize, text: &str, ranges: Vec<(usize, usize)>) -> SearchHit {
        SearchHit { path: PathBuf::from(path), line, col: ranges[0].0, ranges, text: text.into(), preview: text.trim().into() }
    }

    #[gpui::test]
    fn replace_preview_and_events(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::theme::Theme::build(std::sync::Arc::new(bf_core::theme::ThemeData::default()), 0, cx).install(cx));
        let (view, cx) = cx.add_window_view(|window, cx| SearchView::new(Workspace::new(Vec::new()), window, cx));
        let events: Rc<RefCell<Vec<(Vec<PathBuf>, Option<usize>, String, usize)>>> = Default::default();
        let sink = events.clone();
        let _sub = cx.update(|_, cx| {
            cx.subscribe(&view, move |_, e: &SearchEvent, _| {
                if let SearchEvent::Replace { files, line, template, count, .. } = e {
                    sink.borrow_mut().push((files.clone(), *line, template.clone(), *count));
                }
            })
        });
        view.update_in(cx, |v, _, cx| {
            let matcher = bf_text::Matcher::new(&bf_text::Query { text: r"(\w+)\.Count\(\)".into(), regex: true, ..Default::default() }).unwrap().unwrap();
            v.matcher = Some(matcher.clone());
            v.hits = vec![
                hit("/w/A.cs", 3, "    var n = xs.Count();", vec![(12, 22)]),
                hit("/w/A.cs", 9, "a.Count() + b.Count()", vec![(0, 9), (12, 21)]),
                hit("/w/B.cs", 1, "c.Count()", vec![(0, 9)]),
            ];
            v.replace.update(cx, |r, cx| r.set_text("$1.Length".to_owned(), cx));
            let (text, parts) = replace_preview_parts(&v.hits[0], &matcher, "$1.Length");
            assert_eq!(text, "var n = xs.Count()xs.Length;");
            assert_eq!(parts, [(8..18, false), (18..27, true)]);
            v.replace_all(cx);
            v.replace_hit(1, cx);
        });
        let events = events.borrow();
        assert_eq!(events[0], (vec![PathBuf::from("/w/A.cs"), PathBuf::from("/w/B.cs")], None, "$1.Length".to_owned(), 4));
        assert_eq!(events[1], (vec![PathBuf::from("/w/A.cs")], Some(9), "$1.Length".to_owned(), 2));
    }
}
