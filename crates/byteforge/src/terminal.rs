//! The integrated terminal: Alacritty's terminal engine (`alacritty_terminal`: PTY, escape sequence parser,
//! grid and scrollback) running your shell, painted into the panel like the editor paints text.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use alacritty_terminal::event::{Event as TermEvent, EventListener, Notify, OnResize, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Rgb};
use gpui::{
    App, Bounds, ClipboardItem, Context, Corners, ElementId, Entity, EventEmitter, FocusHandle, Focusable, FontStyle, FontWeight,
    GlobalElementId, Hsla, KeyDownEvent, Keystroke, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    ScrollWheelEvent, ShapedLine, SharedString, Style, Task, TextRun, UnderlineStyle, Window, div, fill, point, prelude::*,
    px, quad, relative, size,
};

use crate::actions::{TerminalCopy, TerminalPaste};
use crate::theme::Theme;

pub enum TerminalEvent {
    /// The shell exited.
    Exited,
    TitleChanged,
    /// Ctrl+Click on a file reference: open the file at (1-based line, column; 0 if unknown).
    OpenFile(PathBuf, u32, u32),
}

#[derive(Clone)]
struct Listener(flume::Sender<TermEvent>);

impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        let _ = self.0.send(event);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct GridSize {
    columns: usize,
    lines: usize,
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.lines
    }

    fn screen_lines(&self) -> usize {
        self.lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

/// Cell geometry from the last paint (window pixels), for mouse hit testing.
#[derive(Clone, Copy, Default)]
struct Layout {
    left: f32,
    top: f32,
    cell_width: f32,
    line_height: f32,
}

pub struct TerminalView {
    term: Arc<FairMutex<Term<Listener>>>,
    notifier: Notifier,
    focus: FocusHandle,
    size: GridSize,
    layout: Layout,
    /// The shell's window title (OSC 0/2), or the shell name.
    pub title: String,
    /// A fixed name for terminals started for a task (e.g. "logs: api"), shown instead of the title.
    pub label: Option<String>,
    pub exited: bool,
    /// Resolves relative file references (the folder the shell started in).
    link_root: Option<PathBuf>,
    selecting: bool,
    _events: Task<()>,
}

impl EventEmitter<TerminalEvent> for TerminalView {}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// Shells listed in `/etc/shells` (restricted and special-purpose ones left out), one entry per real
/// program, preferring `/usr/bin` paths.
pub fn installed_shells() -> Vec<String> {
    let text = std::fs::read_to_string("/etc/shells").unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    let mut shells: Vec<String> = Vec::new();
    let mut lines: Vec<&str> = text.lines().map(str::trim).filter(|l| l.starts_with('/')).collect();
    lines.sort_by_key(|l| !l.starts_with("/usr/bin/"));
    for line in lines {
        let name = std::path::Path::new(line).file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if matches!(name, "rbash" | "git-shell" | "nologin" | "false") || name.contains("fallback") {
            continue;
        }
        let real = std::fs::canonicalize(line).unwrap_or_else(|_| PathBuf::from(line));
        if real.is_file() && seen.insert(real) {
            shells.push(line.to_owned());
        }
    }
    shells.sort_by_key(|s| std::path::Path::new(s).file_name().map(|n| n.to_owned()));
    shells
}

/// The shell to start: the setting, else `$SHELL`, else the platform default.
pub fn default_shell(configured: Option<String>) -> Option<String> {
    configured.or_else(|| std::env::var("SHELL").ok()).filter(|s| !s.is_empty())
}

/// A started shell, before its view exists.
pub struct Spawned {
    cwd: Option<PathBuf>,
    term: Arc<FairMutex<Term<Listener>>>,
    notifier: Notifier,
    events: flume::Receiver<TermEvent>,
    size: GridSize,
    title: String,
}

/// Starts `shell` (or the platform default) on a new PTY in `cwd`.
pub fn spawn(shell: Option<String>, args: Vec<String>, cwd: Option<PathBuf>) -> anyhow::Result<Spawned> {
    let size = GridSize { columns: 80, lines: 24 };
    let env = HashMap::from([
        ("TERM".to_owned(), "xterm-256color".to_owned()),
        ("COLORTERM".to_owned(), "truecolor".to_owned()),
        ("TERM_PROGRAM".to_owned(), "ByteForge".to_owned()),
    ]);
    let title = shell
        .as_deref()
        .and_then(|s| std::path::Path::new(s).file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "shell".into());
    let options = tty::Options {
        shell: shell.map(|program| tty::Shell::new(program, args)),
        working_directory: cwd.clone(),
        drain_on_exit: true,
        env,
        #[cfg(windows)]
        escape_args: true,
    };
    let window_size = WindowSize { num_lines: size.lines as u16, num_cols: size.columns as u16, cell_width: 8, cell_height: 16 };
    let pty = tty::new(&options, window_size, 0)?;
    let (tx, events) = flume::unbounded();
    let listener = Listener(tx);
    let term = Arc::new(FairMutex::new(Term::new(Config::default(), &size, listener.clone())));
    let event_loop = EventLoop::new(term.clone(), listener, pty, false, false)?;
    let notifier = Notifier(event_loop.channel());
    event_loop.spawn();
    Ok(Spawned { cwd, term, notifier, events, size, title })
}

impl TerminalView {
    pub fn new(spawned: Spawned, cx: &mut Context<Self>) -> Self {
        let Spawned { cwd, term, notifier, events: rx, size, title } = spawned;
        let events = cx.spawn(async move |this, cx| {
            while let Ok(first) = rx.recv_async().await {
                // Coalesce bursts of output into one repaint.
                let mut batch = vec![first];
                batch.extend(rx.try_iter());
                if this.update(cx, |view, cx| view.on_events(batch, cx)).is_err() {
                    break;
                }
            }
        });
        Self {
            term,
            notifier,
            focus: cx.focus_handle(),
            size,
            layout: Layout::default(),
            title,
            label: None,
            exited: false,
            link_root: cwd,
            selecting: false,
            _events: events,
        }
    }

    fn on_events(&mut self, events: Vec<TermEvent>, cx: &mut Context<Self>) {
        for event in events {
            match event {
                TermEvent::Wakeup | TermEvent::MouseCursorDirty | TermEvent::CursorBlinkingChange => {}
                TermEvent::Title(title) => {
                    self.title = title;
                    cx.emit(TerminalEvent::TitleChanged);
                }
                TermEvent::ResetTitle => {}
                TermEvent::PtyWrite(text) => self.write(text.into_bytes()),
                TermEvent::ClipboardStore(_, text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
                TermEvent::ClipboardLoad(_, format) => {
                    let text = cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
                    self.write(format(&text).into_bytes());
                }
                TermEvent::ColorRequest(index, format) => {
                    let color = self.term.lock().colors()[index].unwrap_or_else(|| default_rgb(index, cx));
                    self.write(format(color).into_bytes());
                }
                TermEvent::TextAreaSizeRequest(format) => {
                    let size = self.window_size();
                    self.write(format(size).into_bytes());
                }
                TermEvent::Bell => {}
                TermEvent::Exit | TermEvent::ChildExit(_) => {
                    if !self.exited {
                        self.exited = true;
                        cx.emit(TerminalEvent::Exited);
                    }
                }
            }
        }
        cx.notify();
    }

    fn window_size(&self) -> WindowSize {
        WindowSize {
            num_lines: self.size.lines as u16,
            num_cols: self.size.columns as u16,
            cell_width: self.layout.cell_width.round() as u16,
            cell_height: self.layout.line_height.round() as u16,
        }
    }

    fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        self.notifier.notify(bytes);
    }

    /// Sends input from the user: scrolls back to the prompt and clears the selection.
    fn input(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        {
            let mut term = self.term.lock();
            term.scroll_display(Scroll::Bottom);
            term.selection = None;
        }
        self.write(bytes);
        cx.notify();
    }

    /// Types `command` into the shell and presses Enter (the shell buffers it until its prompt is ready),
    /// so the user stays in their shell afterwards and can rerun or edit it.
    pub fn run_command(&mut self, command: &str, cx: &mut Context<Self>) {
        let mut bytes = command.as_bytes().to_vec();
        bytes.push(b'\r');
        self.input(bytes, cx);
    }

    /// The tab name: the fixed label, else the shell's title.
    pub fn display_title(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.title)
    }

    fn resize(&mut self, size: GridSize) {
        if size == self.size || size.columns == 0 || size.lines == 0 {
            return;
        }
        self.size = size;
        self.term.lock().resize(size);
        let window_size = self.window_size();
        self.notifier.on_resize(window_size);
    }

    pub fn focus(&self, window: &mut Window) {
        window.focus(&self.focus);
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let mode = *self.term.lock().mode();
        if let Some(bytes) = keystroke_bytes(&event.keystroke, mode.contains(TermMode::APP_CURSOR)) {
            self.input(bytes, cx);
            cx.stop_propagation();
        }
    }

    fn copy(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.term.lock().selection_to_string().filter(|t| !t.is_empty()) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) else { return };
        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        let bytes = if bracketed {
            // Strip the end marker so pasted text can't end bracketed paste early.
            format!("\x1b[200~{}\x1b[201~", text.replace("\x1b[201~", "")).into_bytes()
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
        };
        self.input(bytes, cx);
    }

    /// The grid cell under a window position, plus which half of the cell was hit.
    fn grid_point(&self, x: f32, y: f32) -> (GridPoint, Side) {
        let l = self.layout;
        let col_f = ((x - l.left) / l.cell_width.max(1.0)).max(0.0);
        let col = (col_f as usize).min(self.size.columns.saturating_sub(1));
        let row = (((y - l.top) / l.line_height.max(1.0)).max(0.0) as usize).min(self.size.lines.saturating_sub(1));
        let offset = self.term.lock().grid().display_offset() as i32;
        let side = if col_f.fract() < 0.5 { Side::Left } else { Side::Right };
        (GridPoint::new(Line(row as i32 - offset), Column(col)), side)
    }

    /// The text of the screen row holding `at`, with the char index of `at`'s column.
    fn row_text(&self, at: GridPoint) -> (String, usize) {
        let term = self.term.lock();
        let row = &term.grid()[at.line];
        let mut text = String::new();
        let mut index = 0;
        for col in 0..self.size.columns {
            let cell = &row[Column(col)];
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            if col <= at.column.0 {
                index = text.chars().count();
            }
            text.push(cell.c);
        }
        (text, index)
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        let (x, y) = (f32::from(event.position.x), f32::from(event.position.y));
        let (at, side) = self.grid_point(x, y);
        if event.modifiers.control {
            let (text, col) = self.row_text(at);
            let links = bf_core::links::find_links(&text, self.link_root.as_deref());
            if let Some(link) = links.into_iter().find(|l| l.start <= col && col < l.end) {
                cx.emit(TerminalEvent::OpenFile(link.path, link.line, link.column));
                return;
            }
        }
        let kind = match event.click_count {
            2 => SelectionType::Semantic,
            n if n >= 3 => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        self.term.lock().selection = Some(Selection::new(kind, at, side));
        self.selecting = true;
        cx.notify();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let (at, side) = self.grid_point(f32::from(event.position.x), f32::from(event.position.y));
        if let Some(selection) = self.term.lock().selection.as_mut() {
            selection.update(at, side);
        }
        cx.notify();
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.selecting = false;
        let mut term = self.term.lock();
        // A plain click leaves an empty selection behind; drop it.
        if term.selection.as_ref().is_some_and(|s| s.is_empty()) {
            term.selection = None;
        }
        cx.notify();
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let lh = self.layout.line_height.max(1.0);
        let lines = (f32::from(event.delta.pixel_delta(px(lh)).y) / lh).round() as i32;
        if lines == 0 {
            return;
        }
        let mut term = self.term.lock();
        let mode = *term.mode();
        if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
            // Full-screen programs (less, vim) get arrow keys instead of scrollback.
            drop(term);
            let key = if lines > 0 { "\x1bOA" } else { "\x1bOB" };
            self.write(key.repeat(lines.unsigned_abs() as usize).into_bytes());
        } else {
            term.scroll_display(Scroll::Delta(lines));
        }
        cx.notify();
        cx.stop_propagation();
    }
}

impl Drop for TerminalView {
    fn drop(&mut self) {
        // Ends the PTY reader thread; dropping the PTY hangs up the shell.
        let _ = self.notifier.0.send(Msg::Shutdown);
    }
}

impl Render for TerminalView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::global(cx);
        let pad = t.n("terminal.padding", 6.0);
        div()
            .id("terminal")
            .key_context("Terminal")
            .track_focus(&self.focus)
            .size_full()
            .pl(px(pad))
            .pt(px(pad / 2.0))
            .bg(terminal_background(&t))
            .cursor(gpui::CursorStyle::IBeam)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(|v, _: &TerminalCopy, _, cx| v.copy(cx)))
            .on_action(cx.listener(|v, _: &TerminalPaste, _, cx| v.paste(cx)))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_down(MouseButton::Middle, cx.listener(|v, _, w, cx| {
                v.focus(w);
                v.paste(cx);
            }))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .child(TerminalElement { view: cx.entity() })
    }
}

// ───────────── keyboard ─────────────

/// Translates a keystroke into the bytes a terminal program expects (xterm encoding). `None` for keys
/// the terminal doesn't handle (they keep working as IDE shortcuts).
pub fn keystroke_bytes(keystroke: &Keystroke, app_cursor: bool) -> Option<Vec<u8>> {
    let m = &keystroke.modifiers;
    if m.platform {
        return None;
    }
    // xterm modifier parameter: 1 + shift + 2·alt + 4·ctrl.
    let param = 1 + u8::from(m.shift) + 2 * u8::from(m.alt) + 4 * u8::from(m.control);
    let csi = |code: &str, letter: char| -> Vec<u8> {
        if param > 1 {
            format!("\x1b[{code};{param}{letter}").into_bytes()
        } else if app_cursor && code == "1" {
            format!("\x1bO{letter}").into_bytes()
        } else {
            format!("\x1b[{letter}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> { if param > 1 { format!("\x1b[{n};{param}~") } else { format!("\x1b[{n}~") }.into_bytes() };
    let alt = |bytes: Vec<u8>| if m.alt { [vec![0x1b], bytes].concat() } else { bytes };
    let bytes = match keystroke.key.as_str() {
        "enter" => alt(b"\r".to_vec()),
        "backspace" if m.control => b"\x17".to_vec(),
        "backspace" => alt(b"\x7f".to_vec()),
        "tab" if m.shift => b"\x1b[Z".to_vec(),
        "tab" => b"\t".to_vec(),
        "escape" => b"\x1b".to_vec(),
        "up" => csi("1", 'A'),
        "down" => csi("1", 'B'),
        "right" => csi("1", 'C'),
        "left" => csi("1", 'D'),
        "home" => csi("1", 'H'),
        "end" => csi("1", 'F'),
        "insert" => tilde(2),
        "delete" => tilde(3),
        "pageup" => tilde(5),
        "pagedown" => tilde(6),
        "f1" => b"\x1bOP".to_vec(),
        "f2" => b"\x1bOQ".to_vec(),
        "f3" => b"\x1bOR".to_vec(),
        "f4" => b"\x1bOS".to_vec(),
        "f5" => tilde(15),
        "f6" => tilde(17),
        "f7" => tilde(18),
        "f8" => tilde(19),
        "f9" => tilde(20),
        "f10" => tilde(21),
        "f11" => tilde(23),
        "f12" => tilde(24),
        "space" if m.control => vec![0],
        key if m.control => {
            let mut chars = key.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else { return None };
            let code = match c {
                'a'..='z' => c as u8 - b'a' + 1,
                '@' | '2' => 0,
                '[' | '3' => 0x1b,
                '\\' | '4' => 0x1c,
                ']' | '5' => 0x1d,
                '^' | '6' => 0x1e,
                '_' | '-' | '/' | '7' => 0x1f,
                '8' => 0x7f,
                _ => return None,
            };
            alt(vec![code])
        }
        _ => {
            let text = keystroke.key_char.clone().or_else(|| (keystroke.key.chars().count() == 1).then(|| keystroke.key.clone()))?;
            alt(text.into_bytes())
        }
    };
    Some(bytes)
}

// ───────────── colors ─────────────

/// VS Code's names for the 16 ANSI colors, with VS Code's dark defaults.
const ANSI: [(&str, u32); 16] = [
    ("terminal.ansiBlack", 0x000000),
    ("terminal.ansiRed", 0xcd3131),
    ("terminal.ansiGreen", 0x0dbc79),
    ("terminal.ansiYellow", 0xe5e510),
    ("terminal.ansiBlue", 0x2472c8),
    ("terminal.ansiMagenta", 0xbc3fbc),
    ("terminal.ansiCyan", 0x11a8cd),
    ("terminal.ansiWhite", 0xe5e5e5),
    ("terminal.ansiBrightBlack", 0x666666),
    ("terminal.ansiBrightRed", 0xf14c4c),
    ("terminal.ansiBrightGreen", 0x23d18b),
    ("terminal.ansiBrightYellow", 0xf5f543),
    ("terminal.ansiBrightBlue", 0x3b8eea),
    ("terminal.ansiBrightMagenta", 0xd670d6),
    ("terminal.ansiBrightCyan", 0x29b8db),
    ("terminal.ansiBrightWhite", 0xe5e5e5),
];

fn hsla_to_rgb(c: Hsla) -> Rgb {
    let rgba = c.to_rgb();
    Rgb { r: (rgba.r * 255.0) as u8, g: (rgba.g * 255.0) as u8, b: (rgba.b * 255.0) as u8 }
}

fn rgb(c: Rgb) -> Hsla {
    gpui::rgb(((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32).into()
}

fn terminal_background(t: &Theme) -> Hsla {
    if t.has("terminal.background") {
        t.c("terminal.background")
    } else if t.has("panel.background") {
        t.c("panel.background")
    } else {
        t.c("editor.background")
    }
}

fn terminal_foreground(t: &Theme) -> Hsla {
    if t.has("terminal.foreground") { t.c("terminal.foreground") } else { t.c("editor.foreground") }
}

/// The 256-color palette: ANSI colors from the theme, then the 6×6×6 cube and the gray ramp.
fn palette(index: usize, t: &Theme) -> Hsla {
    match index {
        0..=15 => t.c_or(ANSI[index].0, (ANSI[index].1 << 8) | 0xff),
        16..=231 => {
            let i = index - 16;
            let level = |v: usize| if v == 0 { 0 } else { 55 + v as u32 * 40 };
            gpui::rgb((level(i / 36) << 16) | (level(i / 6 % 6) << 8) | level(i % 6)).into()
        }
        _ => {
            let v = 8 + (index.min(255) - 232) as u32 * 10;
            gpui::rgb((v << 16) | (v << 8) | v).into()
        }
    }
}

fn default_rgb(index: usize, cx: &App) -> Rgb {
    let t = Theme::global(cx);
    hsla_to_rgb(match index {
        256 => terminal_foreground(&t),
        257 => terminal_background(&t),
        258 => t.c_or("terminalCursor.foreground", 0xaeafadff),
        i => palette(i.min(255), &t),
    })
}

struct Colors<'a> {
    theme: &'a Theme,
    overrides: &'a alacritty_terminal::term::color::Colors,
    fg: Hsla,
    bg: Hsla,
}

impl Colors<'_> {
    fn resolve(&self, color: Color) -> Hsla {
        match color {
            Color::Spec(c) => rgb(c),
            Color::Indexed(i) => self.indexed(i as usize),
            Color::Named(name) => {
                let i = name as usize;
                if let Some(c) = self.overrides[i] {
                    return rgb(c);
                }
                match name {
                    NamedColor::Foreground | NamedColor::BrightForeground => self.fg,
                    NamedColor::Background => self.bg,
                    NamedColor::DimForeground => self.fg.opacity(0.7),
                    NamedColor::Cursor => self.theme.c_or("terminalCursor.foreground", 0xaeafadff),
                    _ if i < 16 => self.indexed(i),
                    // Dim variants follow the foreground block (DimBlack = 259).
                    _ => self.indexed((i.saturating_sub(259)).min(7)).opacity(0.7),
                }
            }
        }
    }

    fn indexed(&self, i: usize) -> Hsla {
        self.overrides[i].map(rgb).unwrap_or_else(|| palette(i, self.theme))
    }
}

// ───────────── painting ─────────────

struct TerminalElement {
    view: Entity<TerminalView>,
}

impl IntoElement for TerminalElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

struct Frame {
    lines: Vec<(usize, ShapedLine)>,
    /// (line, first column, column count, color)
    backgrounds: Vec<(usize, usize, usize, Hsla)>,
    cursor: Option<(usize, usize, CursorShape, Hsla)>,
    cell_width: f32,
    line_height: f32,
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = Frame;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App) -> Frame {
        let t = Theme::global(cx);
        let font = gpui::Font { weight: t.editor.font_weight, ..gpui::font(t.editor_font.clone()) };
        let font_size = t.n("terminal.fontSize", t.editor.font_size);
        let text_system = window.text_system().clone();
        let font_id = text_system.resolve_font(&font);
        let cell_width = text_system.advance(font_id, px(font_size), 'm').map(|s| f32::from(s.width)).unwrap_or(8.0).max(1.0);
        let line_height = (font_size * t.n("terminal.lineHeight", 1.2)).round().max(4.0);
        let columns = (f32::from(bounds.size.width) / cell_width).floor().max(2.0) as usize;
        let lines = (f32::from(bounds.size.height) / line_height).floor().max(1.0) as usize;

        let focused = self.view.read(cx).focus.is_focused(window);
        let term = self.view.update(cx, |view, _| {
            view.resize(GridSize { columns, lines });
            view.layout = Layout { left: f32::from(bounds.left()), top: f32::from(bounds.top()), cell_width, line_height };
            view.term.clone()
        });

        let fg = terminal_foreground(&t);
        let bg = terminal_background(&t);
        let selection_bg = t.c_or("terminal.selectionBackground", 0x264f78ff);
        let term = term.lock();
        let content = term.renderable_content();
        let colors = Colors { theme: &t, overrides: content.colors, fg, bg };
        let offset = content.display_offset as i32;
        let mut rows: Vec<(String, Vec<TextRun>)> = vec![(String::new(), Vec::new()); lines];
        let mut backgrounds = Vec::new();
        for indexed in content.display_iter {
            let row = indexed.point.line.0 + offset;
            if row < 0 || row as usize >= lines {
                continue;
            }
            let (row, col) = (row as usize, indexed.point.column.0);
            let cell = indexed.cell;
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            let mut cell_fg = colors.resolve(cell.fg);
            let mut cell_bg = colors.resolve(cell.bg);
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut cell_fg, &mut cell_bg);
            }
            if cell.flags.contains(Flags::DIM) {
                cell_fg = cell_fg.opacity(0.7);
            }
            if cell.flags.contains(Flags::HIDDEN) {
                cell_fg = cell_bg;
            }
            let selected = content.selection.is_some_and(|s| s.contains(indexed.point));
            let paint_bg = if selected { Some(selection_bg) } else { (cell_bg != bg).then_some(cell_bg) };
            if let Some(color) = paint_bg {
                match backgrounds.last_mut() {
                    Some((l, start, len, c)) if *l == row && *start + *len == col && *c == color => *len += 1,
                    _ => backgrounds.push((row, col, 1, color)),
                }
            }
            let (text, runs) = &mut rows[row];
            // Pad skipped columns (wide chars) so text stays on the grid.
            let ch = if cell.c == '\0' || cell.c == '\t' { ' ' } else { cell.c };
            let run_font = gpui::Font {
                weight: if cell.flags.contains(Flags::BOLD) { FontWeight::BOLD } else { font.weight },
                style: if cell.flags.contains(Flags::ITALIC) { FontStyle::Italic } else { FontStyle::Normal },
                ..font.clone()
            };
            let underline = cell.flags.intersects(Flags::ALL_UNDERLINES).then(|| UnderlineStyle { color: Some(cell_fg), thickness: px(1.), wavy: cell.flags.contains(Flags::UNDERCURL) });
            let strikethrough = cell.flags.contains(Flags::STRIKEOUT).then(|| gpui::StrikethroughStyle { color: Some(cell_fg), thickness: px(1.) });
            let len = ch.len_utf8();
            text.push(ch);
            match runs.last_mut() {
                Some(r) if r.color == cell_fg && r.font == run_font && r.underline == underline && r.strikethrough == strikethrough => r.len += len,
                _ => runs.push(TextRun { len, font: run_font, color: cell_fg, background_color: None, underline, strikethrough }),
            }
        }
        let cursor_point = content.cursor.point;
        let cursor_row = cursor_point.line.0 + offset;
        let cursor = (content.cursor.shape != CursorShape::Hidden && cursor_row >= 0 && (cursor_row as usize) < lines).then(|| {
            let shape = if focused { content.cursor.shape } else { CursorShape::HollowBlock };
            (cursor_row as usize, cursor_point.column.0, shape, colors.resolve(Color::Named(NamedColor::Cursor)))
        });
        drop(term);

        let lines = rows
            .into_iter()
            .enumerate()
            .filter(|(_, (text, _))| !text.trim_end().is_empty())
            .map(|(row, (text, runs))| (row, text_system.shape_line(SharedString::from(text), px(font_size), &runs, None)))
            .collect();
        Frame { lines, backgrounds, cursor, cell_width, line_height }
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), frame: &mut Frame, window: &mut Window, cx: &mut App) {
        let (cw, lh) = (frame.cell_width, frame.line_height);
        let origin = bounds.origin;
        let at = |row: usize, col: usize| point(origin.x + px(col as f32 * cw), origin.y + px(row as f32 * lh));
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            for &(row, col, len, color) in &frame.backgrounds {
                window.paint_quad(fill(Bounds::new(at(row, col), size(px(len as f32 * cw), px(lh))), color));
            }
            if let Some((row, col, shape, color)) = frame.cursor {
                let cell = Bounds::new(at(row, col), size(px(cw), px(lh)));
                match shape {
                    CursorShape::Block => window.paint_quad(fill(cell, color.opacity(0.75))),
                    CursorShape::Beam => window.paint_quad(fill(Bounds::new(cell.origin, size(px(2.), px(lh))), color)),
                    CursorShape::Underline => window.paint_quad(fill(Bounds::new(point(cell.origin.x, cell.origin.y + px(lh - 2.)), size(px(cw), px(2.))), color)),
                    _ => window.paint_quad(quad(cell, Corners::default(), Hsla::default(), 1.0, color, gpui::BorderStyle::Solid)),
                }
            }
            for (row, line) in &frame.lines {
                let _ = line.paint(at(*row, 0), px(lh), window, cx);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(key: &str) -> Option<Vec<u8>> {
        keystroke_bytes(&Keystroke::parse(key).unwrap(), false)
    }

    /// The visible text of the terminal grid.
    fn screen(spawned: &Spawned) -> String {
        let term = spawned.term.lock();
        let content = term.renderable_content();
        let mut out = String::new();
        let mut line = 0;
        for cell in content.display_iter {
            if cell.point.line.0 != line {
                out.push('\n');
                line = cell.point.line.0;
            }
            out.push(cell.cell.c);
        }
        out
    }

    #[cfg(unix)]
    #[test]
    fn runs_a_shell_on_a_pty() {
        // The shell sees a real terminal, reads typed input and draws into the grid.
        let spawned = spawn(Some("sh".into()), vec!["-c".into(), "test -t 0 && echo tty-ok; read x; echo got-$x; sleep 1".into()], None).unwrap();
        let wait_for = |needle: &str| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !screen(&spawned).contains(needle) {
                assert!(std::time::Instant::now() < deadline, "{needle:?} never appeared:\n{}", screen(&spawned));
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        };
        wait_for("tty-ok");
        spawned.notifier.notify(keystroke_bytes(&Keystroke::parse("a").unwrap(), false).unwrap());
        spawned.notifier.notify(keystroke_bytes(&Keystroke::parse("enter").unwrap(), false).unwrap());
        wait_for("got-a");
        let _ = spawned.notifier.0.send(Msg::Shutdown);
    }

    #[cfg(unix)]
    #[test]
    fn lists_real_shells_once() {
        let shells = installed_shells();
        let names: Vec<_> = shells.iter().filter_map(|s| std::path::Path::new(s).file_name()?.to_str()).collect();
        assert!(!names.contains(&"rbash") && !names.contains(&"git-shell"), "{shells:?}");
        let mut unique = names.clone();
        unique.dedup();
        assert_eq!(unique.len(), names.len(), "duplicates in {shells:?}");
    }

    #[test]
    fn keys_map_to_xterm_sequences() {
        assert_eq!(bytes("ctrl-c"), Some(vec![3]));
        assert_eq!(bytes("ctrl-w"), Some(vec![0x17]));
        assert_eq!(bytes("enter"), Some(b"\r".to_vec()));
        assert_eq!(bytes("backspace"), Some(vec![0x7f]));
        assert_eq!(bytes("up"), Some(b"\x1b[A".to_vec()));
        assert_eq!(keystroke_bytes(&Keystroke::parse("up").unwrap(), true), Some(b"\x1bOA".to_vec()));
        assert_eq!(bytes("ctrl-right"), Some(b"\x1b[1;5C".to_vec()));
        assert_eq!(bytes("shift-tab"), Some(b"\x1b[Z".to_vec()));
        assert_eq!(bytes("delete"), Some(b"\x1b[3~".to_vec()));
        assert_eq!(bytes("alt-b"), Some(b"\x1bb".to_vec()));
        assert_eq!(bytes("a"), Some(b"a".to_vec()));
    }
}
