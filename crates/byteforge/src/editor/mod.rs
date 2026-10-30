//! The code editor: editing model + highlighting + language-server features. Rendering lives in
//! `element.rs`; this file owns state, input handling and background work.

mod element;

use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bf_core::git::LineChangeKind;
use bf_lsp::{CompletionItem, CompletionKind, LspClient, Position, Severity, TextChange};
use bf_syntax::{Highlighter, Language, Span};
use bf_text::{Buffer, Change, EditorState, Point};
use gpui::{
    App, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ScrollWheelEvent, Subscription, Task, Window, div, prelude::*, px,
};
use parking_lot::Mutex;

pub use element::EditorElement;

use crate::actions::*;
use crate::assets::icon;
use crate::text_input::{InputEvent, TextInput};
use crate::theme::Theme;

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub start: Point,
    pub end: Point,
    pub severity: Severity,
    pub message: String,
    pub code: Option<String>,
}

pub enum EditorEvent {
    /// Text changed (dirty state may have changed).
    Edited,
    CaretMoved,
    DiagnosticsChanged,
    Navigate(PathBuf, Point),
}

struct SyntaxState {
    highlighter: Highlighter,
    pending: Vec<bf_syntax::Edit>,
    full: bool,
}

#[derive(Default)]
struct Spans {
    /// Byte spans sorted by start, non-overlapping.
    list: Vec<Span>,
    /// Byte range the spans cover.
    covered: Range<usize>,
}

struct Completion {
    start: Point,
    items: Arc<Vec<CompletionItem>>,
    filtered: Vec<usize>,
    selected: usize,
}

struct FindState {
    query: Entity<TextInput>,
    replace: Entity<TextInput>,
    show_replace: bool,
    match_case: bool,
    matches: Vec<(Point, Point)>,
    _subs: Vec<Subscription>,
}

#[derive(Clone, Copy, PartialEq)]
enum DragMode {
    None,
    Chars(Point),
    Words(Point, Point),
    Lines(usize),
    VScroll(f32),
    HScroll(f32),
}

/// Geometry from the last paint, used for hit testing (all values in window pixels, as f32).
#[derive(Clone, Copy, Default)]
pub(crate) struct Layout {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
    pub gutter: f32,
    pub text_left: f32,
    pub char_width: f32,
    pub line_height: f32,
}

pub struct EditorView {
    pub state: EditorState,
    pub path: Option<PathBuf>,
    pub language: Option<Language>,
    pub read_only: bool,
    pub show_line_numbers: bool,
    pub show_indent_guides: bool,
    pub smooth_scrolling: bool,
    pub cursor_blinking: bool,
    focus: FocusHandle,

    syntax: Option<Arc<Mutex<SyntaxState>>>,
    syntax_spans: Spans,
    semantic_spans: Spans,
    syntax_task: Option<Task<()>>,
    semantic_task: Option<Task<()>>,

    lsp: Option<LspClient>,
    lsp_version: i32,
    pub diagnostics: Vec<Diagnostic>,
    completion: Option<Completion>,
    completion_task: Option<Task<()>>,
    hover: Option<(Point, String)>,
    hover_task: Option<Task<()>>,
    find: Option<FindState>,

    pub(crate) scroll: (f32, f32),
    pub(crate) scroll_target: (f32, f32),
    pub(crate) caret_drawn: Option<(f32, f32)>,
    pub(crate) last_frame: Option<Instant>,
    pub(crate) blink_epoch: Instant,
    pub(crate) layout: Layout,
    pub(crate) max_line_chars: usize,
    pub(crate) bracket_match: Option<(Point, Option<Point>)>,
    pub(crate) bracket_key: (Point, u64),
    drag: DragMode,
    blink_task: Option<Task<()>>,
    pending_reveal: Option<(Point, bool)>,
    pub(crate) line_cache: std::collections::HashMap<u64, gpui::ShapedLine>,
    pub(crate) depth_cache: Vec<i32>,

    /// Text the gutter change markers compare against (the file at HEAD).
    git_base: Option<Arc<str>>,
    pub(crate) git_marks: Vec<(usize, LineChangeKind)>,
    git_task: Option<Task<()>>,
    /// Whole-line backgrounds as theme color keys (used by the diff view).
    pub line_backgrounds: Vec<(usize, &'static str)>,
}

impl EventEmitter<EditorEvent> for EditorView {}

impl Focusable for EditorView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EditorView {
    pub fn new(text: &str, path: Option<PathBuf>, cx: &mut Context<Self>) -> Self {
        let language = path.as_deref().and_then(Language::from_path);
        let mut state = EditorState::new(Buffer::new(text));
        state.line_comment = match language {
            Some(Language::CSharp) => Some("//"),
            Some(Language::Json) => Some("//"),
            None => None,
        };
        // Parse the buffer's text, not the raw file: CRLF is normalized to LF, and highlight byte offsets
        // must match the text that is displayed.
        let normalized = state.buffer.text();
        let syntax = language.map(|lang| {
            let mut highlighter = Highlighter::new(lang);
            highlighter.parse(&normalized);
            Arc::new(Mutex::new(SyntaxState { highlighter, pending: Vec::new(), full: false }))
        });
        let max_line_chars = text.lines().map(|l| l.chars().count()).max().unwrap_or(0);
        let mut view = Self {
            state,
            path,
            language,
            read_only: false,
            show_line_numbers: true,
            show_indent_guides: true,
            smooth_scrolling: true,
            cursor_blinking: true,
            focus: cx.focus_handle(),
            syntax,
            syntax_spans: Spans::default(),
            semantic_spans: Spans::default(),
            syntax_task: None,
            semantic_task: None,
            lsp: None,
            lsp_version: 0,
            diagnostics: Vec::new(),
            completion: None,
            completion_task: None,
            hover: None,
            hover_task: None,
            find: None,
            scroll: (0.0, 0.0),
            scroll_target: (0.0, 0.0),
            caret_drawn: None,
            last_frame: None,
            blink_epoch: Instant::now(),
            layout: Layout::default(),
            max_line_chars,
            bracket_match: None,
            bracket_key: (Point::new(usize::MAX, 0), u64::MAX),
            drag: DragMode::None,
            blink_task: None,
            pending_reveal: None,
            line_cache: Default::default(),
            depth_cache: Vec::new(),
            git_base: None,
            git_marks: Vec::new(),
            git_task: None,
            line_backgrounds: Vec::new(),
        };
        view.state.buffer.take_changes();
        view.schedule_highlight(cx);
        view.start_blink(cx);
        view
    }

    pub fn is_modified(&self) -> bool {
        self.state.buffer.is_modified()
    }

    pub fn focus(&self, window: &mut Window) {
        window.focus(&self.focus);
    }

    // ───────────── blinking ─────────────

    fn start_blink(&mut self, cx: &mut Context<Self>) {
        self.blink_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let interval = cx
                    .update(|cx| {
                        let style = &Theme::global(cx).editor;
                        match style.caret_blink {
                            crate::theme::CaretBlink::Blink => style.blink_interval_ms.max(100),
                            crate::theme::CaretBlink::Solid => 500,
                            _ => 33,
                        }
                    })
                    .unwrap_or(500);
                cx.background_executor().timer(Duration::from_millis(interval)).await;
                let alive = this
                    .update(cx, |view, cx| {
                        if view.cursor_blinking && Theme::global(cx).editor.caret_blink != crate::theme::CaretBlink::Solid {
                            cx.notify();
                        }
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        }));
    }

    pub(crate) fn caret_opacity(&self, cx: &App) -> f32 {
        let style = &Theme::global(cx).editor;
        if !self.cursor_blinking {
            return 1.0;
        }
        let period = (style.blink_interval_ms.max(100) * 2) as f32 / 1000.0;
        let elapsed = self.blink_epoch.elapsed().as_secs_f32();
        if elapsed < 0.5 {
            return 1.0; // stay solid right after typing/moving
        }
        let t = (elapsed % period) / period;
        match style.caret_blink {
            crate::theme::CaretBlink::Solid => 1.0,
            crate::theme::CaretBlink::Blink => if t < 0.5 { 1.0 } else { 0.0 },
            crate::theme::CaretBlink::Smooth => 0.5 + 0.5 * (t * std::f32::consts::TAU).cos(),
            crate::theme::CaretBlink::Phase => {
                if t < 0.4 { 1.0 } else if t < 0.5 { 1.0 - (t - 0.4) * 10.0 } else if t < 0.9 { 0.0 } else { (t - 0.9) * 10.0 }
            }
        }
    }

    // ───────────── change pipeline ─────────────

    /// Called after every mutation: syncs highlighter, language server, spans, diagnostics and views.
    fn after_edit(&mut self, cx: &mut Context<Self>) {
        let changes = self.state.buffer.take_changes();
        if changes.is_empty() {
            self.after_move(cx);
            return;
        }
        if let Some(min_line) = changes.iter().map(|c| c.start.line).min() {
            self.depth_cache.truncate(min_line + 1);
        }
        for c in &changes {
            shift_spans(&mut self.syntax_spans, c);
            shift_spans(&mut self.semantic_spans, c);
            shift_diagnostics(&mut self.diagnostics, c);
            let delta = c.new_end.line as isize - c.old_end.line as isize;
            if delta != 0 {
                for mark in &mut self.git_marks {
                    if mark.0 > c.old_end.line {
                        mark.0 = (mark.0 as isize + delta).max(0) as usize;
                    }
                }
            }
            let new_len = self.state.buffer.line_len(c.new_end.line);
            if c.start.line == c.old_end.line && c.new_end.line == c.start.line {
                self.max_line_chars = self.max_line_chars.max(new_len);
            } else {
                self.max_line_chars = (0..self.state.buffer.line_count()).map(|l| self.state.buffer.line_len(l)).max().unwrap_or(0);
            }
        }
        if let Some(syntax) = &self.syntax {
            let mut s = syntax.lock();
            if changes.len() > 1 {
                s.full = true;
            }
            for c in &changes {
                s.pending.push(bf_syntax::Edit {
                    start_byte: c.start_byte,
                    old_end_byte: c.old_end_byte,
                    new_end_byte: c.new_end_byte,
                    start_point: c.start_byte_point,
                    old_end_point: c.old_end_byte_point,
                    new_end_point: c.new_end_byte_point,
                });
            }
        }
        if let (Some(lsp), Some(path)) = (&self.lsp, &self.path) {
            self.lsp_version += 1;
            let lsp_changes = changes
                .iter()
                .map(|c| TextChange {
                    range: Some(bf_lsp::Range {
                        start: Position { line: c.start_utf16.0 as u32, character: c.start_utf16.1 as u32 },
                        end: Position { line: c.old_end_utf16.0 as u32, character: c.old_end_utf16.1 as u32 },
                    }),
                    text: c.inserted.clone(),
                })
                .collect();
            lsp.did_change(path, self.lsp_version, lsp_changes);
        }
        self.schedule_highlight(cx);
        self.schedule_semantic(Duration::from_millis(350), cx);
        self.schedule_git_diff(Duration::from_millis(120), cx);
        if let Some(find) = &self.find {
            let (q, mc) = (find.query.read(cx).text().to_owned(), find.match_case);
            if let Some(find) = &mut self.find {
                find.matches = self.state.find_all(&q, mc, 50_000);
            }
        }
        self.state.clamp();
        self.after_move(cx);
        cx.emit(EditorEvent::Edited);
    }

    fn after_move(&mut self, cx: &mut Context<Self>) {
        self.blink_epoch = Instant::now();
        self.ensure_caret_visible();
        if let Some(completion) = &self.completion {
            let caret = self.state.caret();
            if caret.line != completion.start.line || caret.col < completion.start.col {
                self.completion = None;
            } else {
                self.refilter_completion();
            }
        }
        cx.emit(EditorEvent::CaretMoved);
        cx.notify();
    }

    /// Byte range around the viewport that highlighting should cover.
    fn highlight_window(&self) -> Range<usize> {
        let (first, last) = self.visible_lines();
        let b = &self.state.buffer;
        let first = first.saturating_sub(300);
        let last = (last + 300).min(b.line_count() - 1);
        b.point_to_byte(Point::new(first, 0))..b.point_to_byte(Point::new(last, b.line_len(last)))
    }

    pub(crate) fn visible_lines(&self) -> (usize, usize) {
        let lh = self.layout.line_height.max(1.0);
        let height = if self.layout.height > 0.0 { self.layout.height } else { 1200.0 };
        let first = (self.scroll.1 / lh).floor().max(0.0) as usize;
        let last = ((self.scroll.1 + height) / lh).ceil() as usize;
        let max = self.state.buffer.line_count() - 1;
        (first.min(max), last.min(max))
    }

    fn schedule_highlight(&mut self, cx: &mut Context<Self>) {
        let Some(syntax) = self.syntax.clone() else { return };
        let rope = self.state.buffer.rope().clone();
        let version = self.state.buffer.version();
        let range = self.highlight_window();
        let task = cx.background_spawn(async move {
            let text = rope.to_string();
            let mut s = syntax.lock();
            if s.full {
                s.highlighter.parse(&text);
                s.pending.clear();
                s.full = false;
            } else {
                let pending = std::mem::take(&mut s.pending);
                let count = pending.len();
                for (i, edit) in pending.into_iter().enumerate() {
                    if i + 1 == count {
                        s.highlighter.edit(edit, &text);
                    } else {
                        // Intermediate edits only adjust the tree; the final edit reparses once.
                        s.highlighter.edit(edit, &text);
                    }
                }
            }
            let spans = s.highlighter.highlight(&text, range.clone());
            (version, range, spans)
        });
        self.syntax_task = Some(cx.spawn(async move |this, cx| {
            let (version, covered, list) = task.await;
            let _ = this.update(cx, |view, cx| {
                if view.state.buffer.version() == version {
                    view.syntax_spans = Spans { list, covered };
                    cx.notify();
                }
            });
        }));
    }

    /// Re-highlight when the viewport leaves the covered range.
    pub(crate) fn on_viewport_changed(&mut self, cx: &mut Context<Self>) {
        if self.syntax.is_none() {
            return;
        }
        let covered = self.syntax_spans.covered.clone();
        let (first, last) = self.visible_lines();
        let b = &self.state.buffer;
        let need = b.point_to_byte(Point::new(first, 0))..b.point_to_byte(Point::new(last, 0));
        if need.start < covered.start || need.end > covered.end {
            self.schedule_highlight(cx);
        }
    }

    fn schedule_semantic(&mut self, delay: Duration, cx: &mut Context<Self>) {
        let (Some(lsp), Some(path)) = (self.lsp.clone(), self.path.clone()) else { return };
        self.semantic_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let Ok(version) = this.update(cx, |v, _| v.state.buffer.version()) else { return };
            let rx = lsp.semantic_tokens(&path);
            let Ok(Ok(tokens)) = rx.recv_async().await else { return };
            let _ = this.update(cx, |view, cx| {
                if view.state.buffer.version() != version {
                    view.schedule_semantic(Duration::from_millis(200), cx);
                    return;
                }
                let b = &view.state.buffer;
                let mut list: Vec<Span> = tokens
                    .iter()
                    .filter(|t| (t.line as usize) < b.line_count())
                    .map(|t| {
                        let start = b.point_to_byte(b.utf16_to_point(t.line as usize, t.start as usize));
                        let end = b.point_to_byte(b.utf16_to_point(t.line as usize, (t.start + t.length) as usize));
                        Span { start, end, kind: t.kind }
                    })
                    .filter(|s| s.end > s.start)
                    .collect();
                list.sort_by_key(|s| s.start);
                view.semantic_spans = Spans { list, covered: 0..b.len_bytes() };
                cx.notify();
            });
        }));
    }

    /// Kinds for each byte of `line` (syntax colors with semantic colors layered on top).
    pub(crate) fn line_kinds(&self, line: usize, len_bytes: usize) -> Vec<Option<&'static str>> {
        let mut kinds = vec![None; len_bytes.min(20_000)];
        if kinds.is_empty() {
            return kinds;
        }
        let start = self.state.buffer.point_to_byte(Point::new(line, 0));
        let end = start + kinds.len();
        for spans in [&self.syntax_spans, &self.semantic_spans] {
            let first = spans.list.partition_point(|s| s.end <= start);
            for s in &spans.list[first..] {
                if s.start >= end {
                    break;
                }
                let (a, b) = (s.start.max(start) - start, s.end.min(end) - start);
                for k in &mut kinds[a..b] {
                    *k = Some(s.kind);
                }
            }
        }
        kinds
    }

    // ───────────── language server ─────────────

    pub fn attach_lsp(&mut self, lsp: LspClient, cx: &mut Context<Self>) {
        if let Some(path) = &self.path {
            self.lsp_version = 1;
            lsp.did_open(path, self.lsp_version, &self.state.buffer.text());
            self.lsp = Some(lsp);
            self.schedule_semantic(Duration::from_millis(100), cx);
        }
    }

    pub fn detach_lsp(&mut self) {
        if let (Some(lsp), Some(path)) = (self.lsp.take(), &self.path) {
            lsp.did_close(path);
        }
    }

    pub fn set_lsp_diagnostics(&mut self, diagnostics: Vec<bf_lsp::Diagnostic>, cx: &mut Context<Self>) {
        let b = &self.state.buffer;
        self.diagnostics = diagnostics
            .into_iter()
            .map(|d| Diagnostic {
                start: b.utf16_to_point(d.range.start.line as usize, d.range.start.character as usize),
                end: b.utf16_to_point(d.range.end.line as usize, d.range.end.character as usize),
                severity: d.severity,
                message: d.message,
                code: d.code,
            })
            .collect();
        self.diagnostics.sort_by_key(|d| d.start);
        cx.emit(EditorEvent::DiagnosticsChanged);
        cx.notify();
    }

    pub fn did_save(&self) {
        if let (Some(lsp), Some(path)) = (&self.lsp, &self.path) {
            lsp.did_save(path, Some(&self.state.buffer.text()));
        }
    }

    fn lsp_position(&self, p: Point) -> Position {
        let (line, character) = self.state.buffer.point_to_utf16(p);
        Position { line: line as u32, character: character as u32 }
    }

    pub fn go_to_definition_at(&mut self, p: Point, cx: &mut Context<Self>) {
        let (Some(lsp), Some(path)) = (self.lsp.clone(), self.path.clone()) else { return };
        let rx = lsp.definition(&path, self.lsp_position(p));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(locations)) = rx.recv_async().await
                && let Some(loc) = locations.into_iter().next()
            {
                let _ = this.update(cx, |_, cx| {
                    cx.emit(EditorEvent::Navigate(loc.path, Point::new(loc.range.start.line as usize, loc.range.start.character as usize)));
                });
            }
        })
        .detach();
    }

    fn request_hover(&mut self, p: Point, cx: &mut Context<Self>) {
        let (Some(lsp), Some(path)) = (self.lsp.clone(), self.path.clone()) else { return };
        if self.hover.as_ref().is_some_and(|(hp, _)| *hp == p) {
            return;
        }
        let pos = self.lsp_position(p);
        self.hover_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(500)).await;
            let rx = lsp.hover(&path, pos);
            if let Ok(Ok(Some(text))) = rx.recv_async().await {
                let _ = this.update(cx, |view, cx| {
                    view.hover = Some((p, text.chars().take(1200).collect()));
                    cx.notify();
                });
            }
        }));
    }

    // ───────────── completion ─────────────

    fn maybe_trigger_completion(&mut self, typed: &str, cx: &mut Context<Self>) {
        if self.lsp.is_none() {
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

    fn request_completion(&mut self, cx: &mut Context<Self>) {
        let (Some(lsp), Some(path)) = (self.lsp.clone(), self.path.clone()) else { return };
        let caret = self.state.caret();
        let pos = self.lsp_position(caret);
        let version = self.state.buffer.version();
        self.completion_task = Some(cx.spawn(async move |this, cx| {
            let rx = lsp.completion(&path, pos);
            let Ok(Ok(items)) = rx.recv_async().await else { return };
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
                    .unwrap_or_else(|| {
                        let line: Vec<char> = view.state.buffer.line(caret.line).chars().collect();
                        let mut s = caret.col.min(line.len());
                        while s > 0 && (line[s - 1].is_alphanumeric() || line[s - 1] == '_') {
                            s -= 1;
                        }
                        Point::new(caret.line, s)
                    });
                let _ = version;
                view.completion = Some(Completion { start, items: Arc::new(items), filtered: Vec::new(), selected: 0 });
                view.refilter_completion();
                cx.notify();
            });
        }));
    }

    fn refilter_completion(&mut self) {
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

    fn commit_completion(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(completion) = self.completion.take() else { return false };
        let Some(&index) = completion.filtered.get(completion.selected) else { return false };
        let item = &completion.items[index];
        let caret = self.state.caret();
        self.state.replace_range(completion.start, caret, &item.insert_text);
        self.after_edit(cx);
        true
    }

    // ───────────── find ─────────────

    fn open_find(&mut self, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
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

    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find = None;
        window.focus(&self.focus);
        cx.notify();
    }

    fn update_find(&mut self, select: bool, cx: &mut Context<Self>) {
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

    fn find_step(&mut self, forward: bool, cx: &mut Context<Self>) {
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

    fn replace_one(&mut self, cx: &mut Context<Self>) {
        let Some(find) = &self.find else { return };
        let replacement = find.replace.read(cx).text().to_owned();
        let sel = self.state.selection();
        if find.matches.contains(&sel) {
            self.state.insert(&replacement);
            self.after_edit(cx);
        }
        self.find_step(true, cx);
    }

    fn replace_all(&mut self, cx: &mut Context<Self>) {
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

    pub fn reveal(&mut self, p: Point, center: bool) {
        if self.layout.height <= 0.0 {
            self.pending_reveal = Some((p, center));
            return;
        }
        let lh = self.layout.line_height;
        let y = p.line as f32 * lh;
        let mut target = self.scroll_target;
        if center {
            target.1 = y - (self.layout.height - lh) / 2.0;
        } else if y < target.1 {
            target.1 = y;
        } else if y + lh > target.1 + self.layout.height {
            target.1 = y + lh - self.layout.height;
        }
        self.scroll_to(target, false);
    }

    pub fn go_to(&mut self, p: Point, cx: &mut Context<Self>) {
        self.state.set_caret(p, false);
        self.reveal(self.state.caret(), true);
        self.after_move(cx);
    }

    pub(crate) fn take_pending_reveal(&mut self) {
        if let Some((p, center)) = self.pending_reveal.take() {
            self.reveal(p, center);
        }
    }

    fn ensure_caret_visible(&mut self) {
        if self.layout.height <= 0.0 {
            self.pending_reveal = Some((self.state.caret(), false));
            return;
        }
        let caret = self.state.caret();
        self.reveal(caret, false);
        let line = self.state.buffer.line(caret.line);
        let x = bf_text::visual_col(&line, caret.col, self.state.tab_size) as f32 * self.layout.char_width;
        let width = self.layout.width - (self.layout.text_left - self.layout.left);
        let mut target = self.scroll_target;
        if x < target.0 {
            target.0 = (x - self.layout.char_width * 8.0).max(0.0);
        } else if x + self.layout.char_width * 2.0 > target.0 + width {
            target.0 = x + self.layout.char_width * 10.0 - width;
        }
        self.scroll_to(target, false);
    }

    pub(crate) fn clamp_scroll(&self, v: (f32, f32)) -> (f32, f32) {
        let lh = self.layout.line_height.max(1.0);
        let max_y = ((self.state.buffer.line_count() as f32 - 1.0) * lh).max(0.0);
        let content_w = self.max_line_chars as f32 * self.layout.char_width + self.layout.char_width * 8.0;
        let view_w = self.layout.width - (self.layout.text_left - self.layout.left);
        let max_x = (content_w - view_w).max(0.0);
        (v.0.clamp(0.0, max_x), v.1.clamp(0.0, max_y))
    }

    pub(crate) fn scroll_to(&mut self, target: (f32, f32), animate: bool) {
        self.scroll_target = self.clamp_scroll(target);
        if !animate {
            self.scroll = self.scroll_target;
        }
    }

    // ───────────── hit testing ─────────────

    fn point_at(&self, x: f32, y: f32) -> Point {
        let l = &self.layout;
        let lh = l.line_height.max(1.0);
        let line = (((y - l.top) + self.scroll.1) / lh).floor().max(0.0) as usize;
        let line = line.min(self.state.buffer.line_count() - 1);
        let visual = (x - l.text_left + self.scroll.0) / l.char_width.max(1.0);
        Point::new(line, bf_text::logical_col(&self.state.buffer.line(line), visual, self.state.tab_size))
    }

    fn scrollbar_zone(&self, x: f32, y: f32) -> Option<bool> {
        let l = &self.layout;
        let size = 12.0;
        if x >= l.left + l.width - size && y < l.top + l.height - size {
            Some(true)
        } else if y >= l.top + l.height - size && x > l.text_left {
            Some(false)
        } else {
            None
        }
    }

    fn on_mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        self.completion = None;
        let (x, y) = (f32::from(e.position.x), f32::from(e.position.y));
        if let Some(vertical) = self.scrollbar_zone(x, y) {
            self.drag = if vertical { DragMode::VScroll(y) } else { DragMode::HScroll(x) };
            return;
        }
        let p = self.point_at(x, y);
        if e.modifiers.control && !e.modifiers.shift {
            self.state.set_caret(p, false);
            self.after_move(cx);
            self.go_to_definition_at(p, cx);
            return;
        }
        if x < self.layout.left + self.layout.gutter || e.click_count >= 3 {
            let end = if p.line + 1 < self.state.buffer.line_count() { Point::new(p.line + 1, 0) } else { self.state.line_end(p) };
            self.state.select(Point::new(p.line, 0), end);
            self.drag = DragMode::Lines(p.line);
        } else if e.click_count == 2 {
            let (s, w) = self.state.word_at(p);
            self.state.select(s, w);
            self.drag = DragMode::Words(s, w);
        } else {
            let origin = if e.modifiers.shift { self.state.anchor().unwrap_or(self.state.caret()) } else { p };
            if e.modifiers.shift {
                self.state.set_caret(p, true);
            } else {
                self.state.set_caret(p, false);
            }
            self.drag = DragMode::Chars(origin);
        }
        self.after_move(cx);
    }

    fn on_mouse_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let (x, y) = (f32::from(e.position.x), f32::from(e.position.y));
        match self.drag {
            DragMode::None => {
                // Hover: diagnostics immediately, language-server hover after a delay.
                let inside = x > self.layout.text_left && y > self.layout.top && y < self.layout.top + self.layout.height;
                if inside && e.pressed_button.is_none() {
                    let p = self.point_at(x, y);
                    self.request_hover(p, cx);
                } else if self.hover.is_some() {
                    self.hover = None;
                    cx.notify();
                }
                return;
            }
            DragMode::VScroll(last) => {
                let ratio = (self.state.buffer.line_count() as f32 * self.layout.line_height) / self.layout.height.max(1.0);
                self.scroll_to((self.scroll.0, self.scroll.1 + (y - last) * ratio.max(1.0)), false);
                self.drag = DragMode::VScroll(y);
                cx.notify();
                return;
            }
            DragMode::HScroll(last) => {
                let content = self.max_line_chars as f32 * self.layout.char_width;
                let ratio = content / self.layout.width.max(1.0);
                self.scroll_to((self.scroll.0 + (x - last) * ratio.max(1.0), self.scroll.1), false);
                self.drag = DragMode::HScroll(x);
                cx.notify();
                return;
            }
            _ => {}
        }
        if e.pressed_button != Some(MouseButton::Left) {
            self.drag = DragMode::None;
            return;
        }
        let p = self.point_at(x, y);
        match self.drag {
            DragMode::Chars(origin) => self.state.select(origin, p),
            DragMode::Words(ws, we) => {
                let (s, e) = self.state.word_at(p);
                if p < ws { self.state.select(we, s) } else { self.state.select(ws, e) }
            }
            DragMode::Lines(origin) => {
                if p.line < origin {
                    let end = if origin + 1 < self.state.buffer.line_count() { Point::new(origin + 1, 0) } else { self.state.line_end(Point::new(origin, 0)) };
                    self.state.select(end, Point::new(p.line, 0));
                } else {
                    let end = if p.line + 1 < self.state.buffer.line_count() { Point::new(p.line + 1, 0) } else { self.state.line_end(p) };
                    self.state.select(Point::new(origin, 0), end);
                }
            }
            _ => {}
        }
        self.after_move(cx);
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.drag = DragMode::None;
    }

    fn on_scroll(&mut self, e: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let lh = self.layout.line_height.max(1.0);
        let delta = e.delta.pixel_delta(px(lh * 3.0));
        let (mut dx, mut dy) = (-f32::from(delta.x), -f32::from(delta.y));
        if e.modifiers.shift && dx == 0.0 {
            dx = dy;
            dy = 0.0;
        }
        let animate = self.smooth_scrolling && !e.delta.precise();
        let base = if animate { self.scroll_target } else { self.scroll };
        self.scroll_to((base.0 + dx, base.1 + dy), animate);
        self.hover = None;
        self.completion = None;
        self.on_viewport_changed(cx);
        cx.notify();
        cx.stop_propagation();
    }

    // ───────────── actions ─────────────

    fn edit(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut EditorState)) {
        if self.read_only {
            return;
        }
        f(&mut self.state);
        self.after_edit(cx);
    }

    fn mv(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut EditorState)) {
        f(&mut self.state);
        self.after_move(cx);
    }

    fn page_lines(&self) -> isize {
        (self.layout.height / self.layout.line_height.max(1.0)).max(1.0) as isize
    }

    fn completion_nav(&mut self, delta: isize, cx: &mut Context<Self>) -> bool {
        let Some(c) = &mut self.completion else { return false };
        let n = c.filtered.len() as isize;
        if n == 0 {
            return false;
        }
        c.selected = (c.selected as isize + delta).clamp(0, n - 1) as usize;
        cx.notify();
        true
    }

    fn register_actions(&self, d: gpui::Stateful<gpui::Div>, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        d.on_action(cx.listener(|v, _: &MoveLeft, _, cx| v.mv(cx, |s| s.move_left(false, false))))
            .on_action(cx.listener(|v, _: &MoveRight, _, cx| v.mv(cx, |s| s.move_right(false, false))))
            .on_action(cx.listener(|v, _: &SelectLeft, _, cx| v.mv(cx, |s| s.move_left(false, true))))
            .on_action(cx.listener(|v, _: &SelectRight, _, cx| v.mv(cx, |s| s.move_right(false, true))))
            .on_action(cx.listener(|v, _: &MoveWordLeft, _, cx| v.mv(cx, |s| s.move_left(true, false))))
            .on_action(cx.listener(|v, _: &MoveWordRight, _, cx| v.mv(cx, |s| s.move_right(true, false))))
            .on_action(cx.listener(|v, _: &SelectWordLeft, _, cx| v.mv(cx, |s| s.move_left(true, true))))
            .on_action(cx.listener(|v, _: &SelectWordRight, _, cx| v.mv(cx, |s| s.move_right(true, true))))
            .on_action(cx.listener(|v, _: &MoveUp, _, cx| {
                if !v.completion_nav(-1, cx) {
                    v.mv(cx, |s| s.move_vertical(-1, false))
                }
            }))
            .on_action(cx.listener(|v, _: &MoveDown, _, cx| {
                if !v.completion_nav(1, cx) {
                    v.mv(cx, |s| s.move_vertical(1, false))
                }
            }))
            .on_action(cx.listener(|v, _: &SelectUp, _, cx| v.mv(cx, |s| s.move_vertical(-1, true))))
            .on_action(cx.listener(|v, _: &SelectDown, _, cx| v.mv(cx, |s| s.move_vertical(1, true))))
            .on_action(cx.listener(|v, _: &MoveHome, _, cx| v.mv(cx, |s| s.set_caret(s.smart_home(s.caret()), false))))
            .on_action(cx.listener(|v, _: &MoveEnd, _, cx| v.mv(cx, |s| s.set_caret(s.line_end(s.caret()), false))))
            .on_action(cx.listener(|v, _: &SelectHome, _, cx| v.mv(cx, |s| s.set_caret(s.smart_home(s.caret()), true))))
            .on_action(cx.listener(|v, _: &SelectEnd, _, cx| v.mv(cx, |s| s.set_caret(s.line_end(s.caret()), true))))
            .on_action(cx.listener(|v, _: &MoveDocStart, _, cx| v.mv(cx, |s| s.set_caret(Point::default(), false))))
            .on_action(cx.listener(|v, _: &MoveDocEnd, _, cx| v.mv(cx, |s| s.set_caret(s.buffer.end(), false))))
            .on_action(cx.listener(|v, _: &SelectDocStart, _, cx| v.mv(cx, |s| s.set_caret(Point::default(), true))))
            .on_action(cx.listener(|v, _: &SelectDocEnd, _, cx| v.mv(cx, |s| s.set_caret(s.buffer.end(), true))))
            .on_action(cx.listener(|v, _: &PageUp, _, cx| {
                if !v.completion_nav(-8, cx) {
                    let n = v.page_lines();
                    let t = (v.scroll_target.0, v.scroll_target.1 - n as f32 * v.layout.line_height);
                    v.scroll_to(t, v.smooth_scrolling);
                    v.mv(cx, |s| s.move_vertical(-n, false));
                }
            }))
            .on_action(cx.listener(|v, _: &PageDown, _, cx| {
                if !v.completion_nav(8, cx) {
                    let n = v.page_lines();
                    let t = (v.scroll_target.0, v.scroll_target.1 + n as f32 * v.layout.line_height);
                    v.scroll_to(t, v.smooth_scrolling);
                    v.mv(cx, |s| s.move_vertical(n, false));
                }
            }))
            .on_action(cx.listener(|v, _: &SelectPageUp, _, cx| {
                let n = v.page_lines();
                v.mv(cx, |s| s.move_vertical(-n, true))
            }))
            .on_action(cx.listener(|v, _: &SelectPageDown, _, cx| {
                let n = v.page_lines();
                v.mv(cx, |s| s.move_vertical(n, true))
            }))
            .on_action(cx.listener(|v, _: &ScrollLineUp, _, cx| {
                let t = (v.scroll.0, v.scroll.1 - v.layout.line_height);
                v.scroll_to(t, false);
                cx.notify();
            }))
            .on_action(cx.listener(|v, _: &ScrollLineDown, _, cx| {
                let t = (v.scroll.0, v.scroll.1 + v.layout.line_height);
                v.scroll_to(t, false);
                cx.notify();
            }))
            .on_action(cx.listener(|v, _: &Backspace, _, cx| v.edit(cx, |s| s.backspace(false))))
            .on_action(cx.listener(|v, _: &DeleteWordLeft, _, cx| v.edit(cx, |s| s.backspace(true))))
            .on_action(cx.listener(|v, _: &Delete, _, cx| v.edit(cx, |s| s.delete_forward(false))))
            .on_action(cx.listener(|v, _: &DeleteWordRight, _, cx| v.edit(cx, |s| s.delete_forward(true))))
            .on_action(cx.listener(|v, _: &Newline, _, cx| {
                if !v.commit_completion(cx) {
                    v.edit(cx, |s| s.newline())
                }
            }))
            .on_action(cx.listener(|v, _: &Tab, _, cx| {
                if !v.commit_completion(cx) {
                    v.edit(cx, |s| s.tab())
                }
            }))
            .on_action(cx.listener(|v, _: &Outdent, _, cx| v.edit(cx, |s| s.indent_lines(false))))
            .on_action(cx.listener(|v, _: &SelectAll, _, cx| v.mv(cx, |s| s.select_all())))
            .on_action(cx.listener(|v, _: &Copy, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(v.state.copy_text()))))
            .on_action(cx.listener(|v, _: &Cut, _, cx| {
                if v.read_only {
                    return;
                }
                let text = v.state.cut();
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                v.after_edit(cx);
            }))
            .on_action(cx.listener(|v, _: &Paste, _, cx| {
                if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
                    v.edit(cx, |s| s.insert(&text));
                }
            }))
            .on_action(cx.listener(|v, _: &Undo, _, cx| v.edit(cx, |s| s.undo())))
            .on_action(cx.listener(|v, _: &Redo, _, cx| v.edit(cx, |s| s.redo())))
            .on_action(cx.listener(|v, _: &DuplicateLines, _, cx| v.edit(cx, |s| s.duplicate_lines())))
            .on_action(cx.listener(|v, _: &DeleteLines, _, cx| v.edit(cx, |s| s.delete_lines())))
            .on_action(cx.listener(|v, _: &MoveLinesUp, _, cx| v.edit(cx, |s| s.move_lines(false))))
            .on_action(cx.listener(|v, _: &MoveLinesDown, _, cx| v.edit(cx, |s| s.move_lines(true))))
            .on_action(cx.listener(|v, _: &ToggleComment, _, cx| v.edit(cx, |s| s.toggle_comment())))
            .on_action(cx.listener(|v, _: &Find, w, cx| v.open_find(false, w, cx)))
            .on_action(cx.listener(|v, _: &Replace, w, cx| v.open_find(true, w, cx)))
            .on_action(cx.listener(|v, _: &FindNext, _, cx| v.find_step(true, cx)))
            .on_action(cx.listener(|v, _: &FindPrevious, _, cx| v.find_step(false, cx)))
            .on_action(cx.listener(|v, _: &TriggerCompletion, _, cx| v.request_completion(cx)))
            .on_action(cx.listener(|v, _: &GoToDefinition, _, cx| {
                let caret = v.state.caret();
                v.go_to_definition_at(caret, cx);
            }))
            .on_action(cx.listener(|v, _: &Cancel, w, cx| {
                if v.completion.take().is_some() || v.hover.take().is_some() {
                    cx.notify();
                } else if v.find.is_some() {
                    v.close_find(w, cx);
                } else if v.state.has_selection() {
                    v.state.clear_selection();
                    cx.notify();
                } else {
                    cx.propagate();
                }
            }))
    }

    /// Typed text from the platform input handler.
    pub(crate) fn type_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let text: String = text.chars().filter(|c| !c.is_control() || *c == '\t').collect();
        if text.is_empty() {
            return;
        }
        self.state.type_text(&text);
        self.after_edit(cx);
        self.maybe_trigger_completion(&text, cx);
    }

    /// Appends text without undo (used by the Output panel).
    pub fn append(&mut self, text: &str, cx: &mut Context<Self>) {
        let end = self.state.buffer.end();
        let follow = self.state.caret().line + 2 >= self.state.buffer.line_count();
        self.state.buffer.replace(end, end, text);
        self.state.buffer.undo_stack().begin_group();
        self.state.buffer.undo_stack().end_group();
        let lines = self.state.buffer.line_count();
        if lines > 20_000 {
            self.state.buffer.replace(Point::default(), Point::new(lines - 20_000, 0), "");
        }
        self.state.buffer.take_changes();
        self.max_line_chars = self.max_line_chars.max(text.lines().map(|l| l.chars().count()).max().unwrap_or(0));
        if follow {
            let end = self.state.buffer.end();
            self.state.set_caret(end, false);
            self.reveal(end, false);
        }
        cx.notify();
    }

    pub fn save_path_text(&self) -> String {
        self.state.buffer.text_for_save()
    }

    /// Replaces the content with a newer version from disk (keeps the caret where possible).
    pub fn reload_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.state.buffer.set_text(text);
        self.state.clamp();
        self.after_edit(cx);
    }

    // ───────────── git gutter ─────────────

    /// Sets the text that change markers compare against (`None` hides the markers).
    pub fn set_git_base(&mut self, base: Option<String>, cx: &mut Context<Self>) {
        if self.git_base.as_deref() == base.as_deref() {
            return;
        }
        self.git_base = base.map(Arc::from);
        self.schedule_git_diff(Duration::ZERO, cx);
    }

    fn schedule_git_diff(&mut self, delay: Duration, cx: &mut Context<Self>) {
        let Some(base) = self.git_base.clone() else {
            if !self.git_marks.is_empty() {
                self.git_marks.clear();
                cx.notify();
            }
            return;
        };
        let rope = self.state.buffer.rope().clone();
        let version = self.state.buffer.version();
        self.git_task = Some(cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let marks = cx.background_executor().spawn(async move { bf_core::git::line_changes(&base, &rope.to_string()) }).await;
            let _ = this.update(cx, |view, cx| {
                if view.state.buffer.version() == version {
                    view.git_marks = marks;
                    cx.notify();
                }
            });
        }));
    }

}

impl Drop for EditorView {
    fn drop(&mut self) {
        self.detach_lsp();
    }
}

fn subsequence(text: &str, query: &str) -> bool {
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

/// Keeps stale spans aligned with the text until fresh results arrive (no flicker while typing).
fn shift_spans(spans: &mut Spans, c: &Change) {
    if spans.list.is_empty() {
        return;
    }
    let delta = c.new_end_byte as isize - c.old_end_byte as isize;
    let single_line = c.start.line == c.old_end.line && !c.inserted.contains('\n');
    let shift = |x: usize| (x as isize + delta).max(0) as usize;
    let mut out = Vec::with_capacity(spans.list.len());
    for s in spans.list.drain(..) {
        if s.end <= c.start_byte {
            if single_line && s.end == c.start_byte && s.end > s.start && c.removed.is_empty() {
                out.push(Span { end: shift(s.end), ..s }); // typing at the end of a token extends it
            } else {
                out.push(s);
            }
        } else if s.start >= c.old_end_byte {
            out.push(Span { start: shift(s.start), end: shift(s.end), kind: s.kind });
        } else if s.start < c.start_byte && s.end >= c.old_end_byte && single_line {
            out.push(Span { end: shift(s.end), ..s });
        } else if s.start < c.start_byte {
            out.push(Span { end: c.start_byte, ..s });
        }
    }
    spans.list = out;
    spans.covered = spans.covered.start..shift(spans.covered.end);
}

fn shift_diagnostics(diags: &mut [Diagnostic], c: &Change) {
    let line_delta = c.new_end.line as isize - c.old_end.line as isize;
    let shift = |p: Point| -> Point {
        if p < c.old_end {
            if p < c.start { p } else { c.start }
        } else if p.line == c.old_end.line {
            Point::new(c.new_end.line, c.new_end.col + (p.col - c.old_end.col))
        } else {
            Point::new((p.line as isize + line_delta).max(0) as usize, p.col)
        }
    };
    for d in diags {
        d.start = shift(d.start);
        d.end = shift(d.end);
    }
}

impl Render for EditorView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let root = div()
            .id("editor")
            .key_context("Editor")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .overflow_hidden()
            .cursor(gpui::CursorStyle::IBeam)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_scroll_wheel(cx.listener(Self::on_scroll));
        let root = self.register_actions(root, cx).child(EditorElement::new(cx.entity()));

        let mut root = root;
        if let Some(find) = &self.find {
            root = root.child(self.render_find(find, cx));
        }
        if let Some(popup) = self.render_completion(window, cx) {
            root = root.child(popup);
        }
        if let Some(popup) = self.render_hover(cx) {
            root = root.child(popup);
        }
        root
    }
}

impl EditorView {
    fn caret_screen_pos(&self, p: Point) -> (f32, f32) {
        let line = self.state.buffer.line(p.line);
        let x = self.layout.text_left - self.scroll.0 + bf_text::visual_col(&line, p.col, self.state.tab_size) as f32 * self.layout.char_width;
        let y = self.layout.top + p.line as f32 * self.layout.line_height - self.scroll.1;
        (x - self.layout.left, y - self.layout.top)
    }

    fn render_find(&self, find: &FindState, cx: &mut Context<Self>) -> impl IntoElement {
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

    fn render_completion(&self, _: &mut Window, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
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

    fn render_hover(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let t = Theme::global(cx);
        let (p, lsp_text) = self.hover.clone()?;
        let diag: Vec<String> = self
            .diagnostics
            .iter()
            .filter(|d| p >= d.start && (p <= d.end || (d.start == d.end && p.line == d.start.line)))
            .map(|d| match &d.code {
                Some(code) => format!("{} ({code})", d.message),
                None => d.message.clone(),
            })
            .collect();
        let text = diag.into_iter().chain(std::iter::once(lsp_text).filter(|s| !s.trim().is_empty())).collect::<Vec<_>>().join("\n\n");
        if text.is_empty() {
            return None;
        }
        let (x, y) = self.caret_screen_pos(p);
        Some(
            div()
                .absolute()
                .left(px(x.max(4.0)))
                .top(px((y - 8.0).max(0.0)))
                .max_w(px(560.))
                .bg(t.bg("editorHoverWidget.background"))
                .text_color(t.c("editorHoverWidget.foreground"))
                .border_color(t.c("editorHoverWidget.border"))
                .border_1()
                .rounded(t.radius("widget.radius", 6.0))
                .shadow(t.shadow("widget.shadow"))
                .p_2()
                .text_size(t.m("ui.size", 13.0))
                .font_family(t.ui_font.clone())
                .child(text),
        )
    }
}

fn kind_glyph(kind: CompletionKind) -> (&'static str, &'static str) {
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
