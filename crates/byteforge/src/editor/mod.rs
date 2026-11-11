//! The code editor: editing model + highlighting + language-server features. Rendering lives in
//! `element.rs`; this file owns state, editing and background work. Language-server features, completion,
//! find, scrolling, mouse input, action bindings and popups live in their own submodules.

mod bindings;
mod completion;
mod element;
mod find;
mod folding;
mod inlays;
mod lsp;
mod merge;
mod minimap;
mod mouse;
mod popups;
mod refactor;
mod snippet;
mod viewport;

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
pub use refactor::{AfterApply, generated_name, invents_name};


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
    /// The document's symbols were read again (breadcrumbs).
    OutlineChanged,
    DiagnosticsChanged,
    Navigate(PathBuf, Point),
    /// Breakpoints were toggled, or edits moved them.
    BreakpointsChanged,
    /// A code action (quick fix, refactoring) was picked; the workbench resolves and applies it, then does
    /// the follow-up.
    ApplyCodeAction(Box<bf_lsp::CodeAction>, AfterApply),
    /// Several results to list in the References panel (Find All References, implementations, …), e.g.
    /// `title` = "references to 'Count'".
    Locations { title: String, locations: Vec<bf_lsp::Location> },
    /// Rename Symbol (F2) on `symbol` at `at`: the workbench asks for the new name, then calls
    /// [`EditorView::rename_symbol`].
    RenameRequested { symbol: String, at: Point },
    /// The language server's edits for a rename, to apply across files.
    ApplyWorkspaceEdit(Box<bf_lsp::WorkspaceEdit>),
    /// "Show Commit of Line": open what `commit` changed in the file (`filename`: its path then, relative to
    /// the repository).
    ShowCommit { commit: String, filename: String },
    /// A status message for the Output panel (e.g. "nothing to rename here").
    Log(String),
    /// The test marker in the gutter on this line was clicked: run that test.
    RunTestAt(usize),
}

/// A breakpoint in the gutter (0-based line). `verified` is false while the debugger couldn't bind it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Breakpoint {
    pub line: usize,
    pub verified: bool,
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

/// A hover popup for the word in `range` (end exclusive, one line).
struct Hover {
    range: (Point, Point),
    /// Markdown from the language server (empty when only diagnostics are shown).
    text: String,
}

/// Parameter hints for the call around the caret.
struct SignatureState {
    help: bf_lsp::SignatureHelp,
    /// Overload shown (starts at the server's active signature; the arrows switch).
    selected: usize,
}

/// The Quick Fix menu (Ctrl+.), the Refactor menu (Ctrl+Shift+R), or the variants of one refactoring.
struct ActionMenu {
    at: Point,
    /// `None` while the server is asked.
    items: Option<Vec<bf_lsp::CodeAction>>,
    selected: usize,
    mode: refactor::MenuMode,
    /// Shown instead of the default when there is nothing to list.
    empty: Option<String>,
}

/// The editor's right-click menu, at a window position.
struct ContextMenu {
    position: gpui::Point<gpui::Pixels>,
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
    Minimap,
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
    /// Width of the breakpoint margin at the left of the gutter (0 when hidden).
    pub glyph: f32,
    /// Left edge and width of the folding chevron column in the gutter (width 0 when off).
    pub fold_left: f32,
    pub fold_width: f32,
    /// Width of the minimap at the right edge (0 when hidden).
    pub minimap: f32,
}

impl Layout {
    /// Width of the text area: right of the gutter, left of the minimap.
    pub fn text_width(&self) -> f32 {
        let reserved = if self.minimap > 0.0 { self.minimap + minimap::SCROLLBAR_GAP } else { 0.0 };
        self.width - (self.text_left - self.left) - reserved
    }
}

/// A shared vertical scroll target: `(y, animate, generation)`. Each editor adopts a generation it hasn't seen.
pub type ScrollLink = std::rc::Rc<std::cell::Cell<(f32, bool, u64)>>;

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
    /// A second view of a file open in another group: it asks the server (completion, hover, …) but
    /// leaves the document's sync (open, changes, close) to the other view, which gets its edits.
    pub lsp_mirror: bool,
    pub diagnostics: Vec<Diagnostic>,
    completion: Option<Completion>,
    completion_task: Option<Task<()>>,
    hover: Option<Hover>,
    signature: Option<SignatureState>,
    signature_task: Option<Task<()>>,
    /// Uses of the symbol under the caret.
    pub(crate) word_highlights: Vec<inlays::WordHighlight>,
    highlight_task: Option<Task<()>>,
    /// Folded regions and the line ↔ screen row mapping.
    pub(crate) folds: folding::Folds,
    /// Shows folding chevrons in the gutter ("editor.folding").
    pub folding_enabled: bool,
    /// The caret's line before the last move (to step over folds with the arrow keys).
    last_caret_line: usize,
    /// Snippets offered in completion (built-in C# ones plus the user's).
    pub snippets: Arc<Vec<bf_core::snippets::Snippet>>,
    /// The snippet being filled in: Tab goes to its next field.
    snippet_session: Option<snippet::Session>,
    /// Inlay hints sorted by position ("editor.inlayHints").
    pub(crate) inlays: Vec<inlays::Inlay>,
    pub inlay_hints_enabled: bool,
    pub minimap_enabled: bool,
    /// Test Explorer states of the tests (and test classes) declared in this file, by line.
    test_marks: Vec<(usize, crate::testing::TestState)>,
    minimap: Option<Arc<minimap::MiniMap>>,
    minimap_hover: bool,
    inlay_task: Option<Task<()>>,
    action_menu: Option<ActionMenu>,
    action_task: Option<Task<()>>,
    context_menu: Option<ContextMenu>,
    /// Blame of the file (the caret line's annotation).
    blame: merge::BlameState,
    /// The document's symbols (for the breadcrumbs), and the task refreshing them.
    outline: Vec<bf_lsp::Symbol>,
    outline_task: Option<Task<()>>,
    /// Merge conflicts in the text.
    conflicts: Vec<bf_core::conflicts::Conflict>,
    /// The word a hover request is waiting for.
    hover_pending: Option<(Point, Point)>,
    hover_task: Option<Task<()>>,
    /// Where the hover popup was painted, so moving the pointer onto it doesn't close it.
    hover_bounds: std::rc::Rc<std::cell::Cell<Option<gpui::Bounds<gpui::Pixels>>>>,
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
    /// Character-range backgrounds as theme color keys (the diff view's changed words).
    pub range_backgrounds: Vec<(Point, Point, &'static str)>,
    /// The number shown in the gutter for each line instead of its own (`None`: no number). The diff view
    /// shows the file's line numbers, with none on the filler lines that keep both sides aligned.
    pub line_numbers: Option<Arc<Vec<Option<usize>>>>,
    /// Editors sharing one of these scroll together vertically (the two sides of a diff).
    pub(crate) scroll_link: Option<ScrollLink>,
    scroll_link_seen: u64,
    /// Shows the breakpoint margin at the left of the gutter; clicking it toggles breakpoints.
    pub glyph_margin: bool,
    /// Sorted by line, one per line.
    pub breakpoints: Vec<Breakpoint>,
    /// Where the debugger is paused: (0-based line, whether it's the top frame).
    pub exec_line: Option<(usize, bool)>,
    /// Underlines file references (`File.cs(12,5)`, `path:line`) and opens them on click (Ctrl+Click in
    /// editable views). Relative paths resolve against `link_root`.
    pub detect_links: bool,
    pub link_root: Option<PathBuf>,
    /// File references per line text, so the file system is only asked once per distinct line.
    pub(crate) link_cache: std::collections::HashMap<String, Arc<Vec<bf_core::links::FileLink>>>,
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
        state.line_comment = language.and_then(Language::line_comment);
        state.block_comment = match language {
            Some(Language::Razor) => Some(("@*", "*@")),
            Some(Language::Xml) => Some(("<!--", "-->")),
            _ => None,
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
            lsp_mirror: false,
            diagnostics: Vec::new(),
            completion: None,
            signature: None,
            signature_task: None,
            word_highlights: Vec::new(),
            highlight_task: None,
            folds: folding::Folds::default(),
            folding_enabled: true,
            last_caret_line: 0,
            snippets: Arc::new(Vec::new()),
            snippet_session: None,
            inlays: Vec::new(),
            inlay_hints_enabled: true,
            minimap_enabled: false,
            test_marks: Vec::new(),
            minimap: None,
            minimap_hover: false,
            inlay_task: None,
            completion_task: None,
            hover: None,
            action_menu: None,
            action_task: None,
            context_menu: None,
            blame: Default::default(),
            conflicts: Vec::new(),
            outline: Vec::new(),
            outline_task: None,
            hover_pending: None,
            hover_task: None,
            hover_bounds: Default::default(),
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
            range_backgrounds: Vec::new(),
            line_numbers: None,
            scroll_link: None,
            scroll_link_seen: 0,
            glyph_margin: false,
            breakpoints: Vec::new(),
            exec_line: None,
            detect_links: false,
            link_root: None,
            link_cache: Default::default(),
        };
        view.refresh_conflicts();
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
        let breakpoints_before = self.breakpoints.clone();
        for c in &changes {
            shift_spans(&mut self.syntax_spans, c);
            shift_spans(&mut self.semantic_spans, c);
            shift_diagnostics(&mut self.diagnostics, c);
            self.shift_decorations(c);
            self.folds.shift(c);
            if let Some(session) = &mut self.snippet_session {
                session.shift(c);
            }
            shift_breakpoints(&mut self.breakpoints, c);
            if let Some((line, _)) = &mut self.exec_line
                && *line > c.old_end.line
            {
                *line = (*line as isize + c.new_end.line as isize - c.old_end.line as isize).max(0) as usize;
            }
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
        if let (Some(lsp), Some(path)) = (&self.lsp, &self.path)
            && !self.lsp_mirror
        {
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
        self.schedule_inlay_hints(Duration::from_millis(600), cx);
        self.schedule_git_diff(Duration::from_millis(120), cx);
        self.after_edit_merge(cx);
        self.schedule_outline(Duration::from_millis(1000), cx);
        if let Some(find) = &self.find {
            let (q, mc) = (find.query.read(cx).text().to_owned(), find.match_case);
            if let Some(find) = &mut self.find {
                find.matches = self.state.find_all(&q, mc, 50_000);
            }
        }
        self.state.clamp();
        self.clear_hover(cx);
        self.action_menu = None;
        self.after_move(cx);
        cx.emit(EditorEvent::Edited);
        if self.breakpoints != breakpoints_before {
            cx.emit(EditorEvent::BreakpointsChanged);
        }
    }

    // ───────────── breakpoints ─────────────

    /// Adds or removes the breakpoint on `line`.
    pub fn toggle_breakpoint(&mut self, line: usize, cx: &mut Context<Self>) {
        if line >= self.state.buffer.line_count() {
            return;
        }
        match self.breakpoints.binary_search_by_key(&line, |b| b.line) {
            Ok(i) => {
                self.breakpoints.remove(i);
            }
            Err(i) => self.breakpoints.insert(i, Breakpoint { line, verified: true }),
        }
        cx.emit(EditorEvent::BreakpointsChanged);
        cx.notify();
    }

    pub fn set_breakpoints(&mut self, mut breakpoints: Vec<Breakpoint>, cx: &mut Context<Self>) {
        breakpoints.sort_by_key(|b| b.line);
        breakpoints.dedup_by_key(|b| b.line);
        if breakpoints != self.breakpoints {
            self.breakpoints = breakpoints;
            cx.notify();
        }
    }

    pub fn set_test_marks(&mut self, marks: Vec<(usize, crate::testing::TestState)>, cx: &mut Context<Self>) {
        if marks != self.test_marks {
            self.test_marks = marks;
            cx.notify();
        }
    }

    pub fn set_exec_line(&mut self, exec_line: Option<(usize, bool)>, cx: &mut Context<Self>) {
        if self.exec_line != exec_line {
            self.exec_line = exec_line;
            if let Some((line, _)) = exec_line {
                self.reveal(Point::new(line, 0), true);
            }
            cx.notify();
        }
    }

    fn after_move(&mut self, cx: &mut Context<Self>) {
        if !self.folds.is_empty() {
            self.caret_into_fold(self.last_caret_line);
        }
        self.last_caret_line = self.state.caret().line;
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
        if self.signature.is_some() {
            self.request_signature_help(cx);
        }
        self.schedule_word_highlights(cx);
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
        // Rows to lines: folded lines between them aren't drawn.
        (self.folds.line_of(first).min(max), self.folds.line_of(last).min(max))
    }

    /// Re-detects the language from the file name (a module's language appeared or went away) and
    /// re-highlights when it changed.
    pub fn redetect_language(&mut self, cx: &mut Context<Self>) {
        let language = self.path.as_deref().and_then(Language::from_path);
        self.set_language(language, cx);
    }

    /// Highlights the text as `language` (for views without a file, like the sides of a diff).
    pub fn set_language(&mut self, language: Option<Language>, cx: &mut Context<Self>) {
        if language == self.language {
            return;
        }
        self.language = language;
        self.state.line_comment = language.and_then(Language::line_comment);
        let text = self.state.buffer.text();
        self.syntax = language.map(|lang| {
            let mut highlighter = Highlighter::new(lang);
            highlighter.parse(&text);
            Arc::new(Mutex::new(SyntaxState { highlighter, pending: Vec::new(), full: false }))
        });
        self.syntax_spans = Spans::default();
        self.schedule_highlight(cx);
        cx.notify();
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
        if self.signature.is_none() && text.ends_with(['(', ',', '<']) {
            self.request_signature_help(cx);
        }
    }

    /// Inserts text at every caret, replacing the selections (modules' `insertText`).
    pub fn insert_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        self.state.insert(text);
        self.after_edit(cx);
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

/// Moves breakpoints with their lines: lines below the change shift, a line pushed down by text
/// inserted at its start moves with it, and breakpoints on deleted lines collapse onto the change.
fn shift_breakpoints(breakpoints: &mut Vec<Breakpoint>, c: &Change) {
    if breakpoints.is_empty() {
        return;
    }
    let delta = c.new_end.line as isize - c.old_end.line as isize;
    for b in breakpoints.iter_mut() {
        let pushed_down = b.line == c.start.line && c.start.col == 0 && c.removed.is_empty() && c.inserted.contains('\n');
        if b.line > c.old_end.line || pushed_down {
            b.line = (b.line as isize + delta).max(0) as usize;
        } else if b.line > c.start.line {
            b.line = c.start.line;
        }
    }
    breakpoints.sort_by_key(|b| b.line);
    breakpoints.dedup_by_key(|b| b.line);
}

fn shift_diagnostics(diags: &mut [Diagnostic], c: &Change) {
    for d in diags {
        d.start = inlays::shift_point(d.start, c);
        d.end = inlays::shift_point(d.end, c);
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
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_right_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_hover(cx.listener(|view, hovered: &bool, _, cx| {
                if !hovered {
                    view.clear_hover(cx);
                }
            }))
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
        if let Some(popup) = self.render_signature(cx) {
            root = root.child(popup);
        }
        if let Some(popup) = self.render_action_menu(cx) {
            root = root.child(popup);
        }
        if let Some(menu) = self.render_context_menu(cx) {
            root = root.child(menu);
        }
        root
    }
}

impl EditorView {
    fn caret_screen_pos(&self, p: Point) -> (f32, f32) {
        let line = self.state.buffer.line(p.line);
        let x = self.layout.text_left - self.scroll.0 + (bf_text::visual_col(&line, p.col, self.state.tab_size) + self.inlay_shift(p.line, p.col, false)) as f32 * self.layout.char_width;
        let y = self.layout.top + self.folds.row_of(p.line) as f32 * self.layout.line_height - self.scroll.1;
        (x - self.layout.left, y - self.layout.top)
    }

}
