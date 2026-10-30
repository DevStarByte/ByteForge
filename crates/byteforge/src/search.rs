//! Search in files: streaming background search with a virtualized result list.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bf_core::Workspace;
use bf_core::search::{SearchHit, SearchMessage, search};
use bf_text::Point;
use gpui::{App, Context, Entity, EventEmitter, FocusHandle, Focusable, Subscription, Task, Window, div, prelude::*, px, uniform_list};

use crate::text_input::{InputEvent, TextInput};
use crate::theme::Theme;

pub enum SearchEvent {
    Open(PathBuf, Point),
}

pub struct SearchView {
    workspace: Workspace,
    input: Entity<TextInput>,
    match_case: bool,
    hits: Vec<SearchHit>,
    summary: String,
    cancel: Arc<AtomicBool>,
    task: Option<Task<()>>,
    _sub: Subscription,
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
        let sub = cx.subscribe_in(&input, window, |this, _, event, _, cx| {
            if let InputEvent::Confirmed = event {
                this.run(cx);
            }
        });
        Self { workspace, input, match_case: false, hits: Vec::new(), summary: String::new(), cancel: Arc::new(AtomicBool::new(false)), task: None, _sub: sub }
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
        self.summary = "Searching…".into();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = cancel.clone();
        let workspace = self.workspace.clone();
        let match_case = self.match_case;
        self.task = Some(cx.spawn(async move |this, cx| {
            let files = cx.background_executor().spawn(async move { workspace.files() }).await;
            let rx = search(files, query, match_case, cancel);
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
        let list = uniform_list("search-results", self.hits.len(), move |range, _, cx| {
            let view = entity.read(cx);
            let t = Theme::global(cx);
            range
                .map(|ix| {
                    let hit = &view.hits[ix];
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
                        .child(div().whitespace_nowrap().overflow_hidden().child(hit.preview.clone()))
                        .child(div().whitespace_nowrap().overflow_hidden().text_size(px(11.)).text_color(t.c("foreground.muted")).child(format!("{rel}:{}", hit.line + 1)))
                        .on_click(move |_, _, cx| entity.update(cx, |_, cx| cx.emit(SearchEvent::Open(path.clone(), point))))
                })
                .collect()
        })
        .flex_1();

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
                    .child(div().flex_1().child(self.input.clone()))
                    .child(
                        div()
                            .id("search-case")
                            .px_1()
                            .rounded(px(3.))
                            .cursor_pointer()
                            .when(self.match_case, |d| d.bg(t.bg("button.background")).text_color(t.c("button.foreground")))
                            .child("Aa")
                            .on_click(cx.listener(|v, _, _, cx| {
                                v.match_case = !v.match_case;
                                v.run(cx);
                            })),
                    ),
            )
            .child(div().px_3().text_size(px(12.)).text_color(t.c("foreground.muted")).child(self.summary.clone()))
            .child(list)
    }
}
