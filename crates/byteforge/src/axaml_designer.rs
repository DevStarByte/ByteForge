//! The Avalonia designer: an `.axaml` view drawn by ByteForge with a toolbox, the document outline and the
//! property grid. Controls are dropped into panels (a cell of a Grid, a place in a StackPanel, a side of
//! a DockPanel, a point on a Canvas), moved, resized and deleted with the mouse; every change is written
//! into the file through its own editor, so typing in the XAML shows here at once (and the other way round),
//! and saving, undo and the tab's modified mark work as for any file.
//!
//! The view is laid out the way Avalonia lays it out, and controls look like the Fluent theme's, light or
//! dark; styles and resources aren't applied.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use bf_core::avalonia::{self as ax, Doc, DropPlan, ElPath, Element, Kind, Layout, Placed, Prop, Rect, Role};
use gpui::{
    AnyElement, App, ClickEvent, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable, Hsla, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, SharedString, Subscription, Window, div, prelude::*, px,
};

use crate::actions::{Copy, Cut, Paste, Redo, Undo, ViewCode};
use crate::editor::{EditorEvent, EditorView};
use crate::text_input::{InputEvent, TextInput};
use crate::theme::Theme;

pub enum AxamlEvent {
    /// F7: the XAML (at this 0-based line).
    ViewCode { line: Option<usize> },
    /// Go to (or first create) an event handler in the code-behind.
    Handler { method: String, args: String, namespace: String },
}

/// A toolbox entry dragged onto the view.
#[derive(Clone)]
pub struct DraggedControl(pub &'static str);

impl Render for DraggedControl {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::global(cx);
        div()
            .px_2()
            .py_1()
            .rounded(px(4.))
            .bg(t.bg("menu.background"))
            .border_1()
            .border_color(t.c("menu.border"))
            .text_color(t.c("menu.foreground"))
            .text_size(t.m("ui.size", 13.0))
            .child(self.0)
    }
}

/// A resize handle: which side (-1, 0, 1) on each axis.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Handle(i8, i8);

const ALL_HANDLES: [Handle; 8] = [Handle(-1, -1), Handle(0, -1), Handle(1, -1), Handle(-1, 0), Handle(1, 0), Handle(-1, 1), Handle(0, 1), Handle(1, 1)];
const ROOT_HANDLES: [Handle; 3] = [Handle(1, 0), Handle(0, 1), Handle(1, 1)];

enum DragKind {
    /// Moving a control, held at `grab` from its top left.
    Move { path: ElPath, grab: (f32, f32), rect: Rect },
    Resize { path: ElPath, handle: Handle, rect: Rect },
    Create { tag: &'static str },
}

struct Drag {
    kind: DragKind,
    start: (f32, f32),
    now: (f32, f32),
    moved: bool,
}

/// A row of the property grid.
#[derive(Clone)]
enum Row {
    /// x:Name.
    Name,
    Known(&'static Prop),
    /// An attribute the grid doesn't know.
    Other(String),
}

impl Row {
    fn attr(&self) -> String {
        match self {
            Row::Name => "x:Name".into(),
            Row::Known(p) => p.name.into(),
            Row::Other(n) => n.clone(),
        }
    }

    fn label(&self) -> String {
        match self {
            Row::Name => "(Name)".into(),
            _ => self.attr(),
        }
    }
}

impl PartialEq for Row {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Row::Name, Row::Name) => true,
            (Row::Known(a), Row::Known(b)) => std::ptr::eq(*a, *b),
            (Row::Other(a), Row::Other(b)) => a == b,
            _ => false,
        }
    }
}

impl std::fmt::Debug for Row {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.attr())
    }
}

#[derive(Clone, Copy, PartialEq)]
enum LeftTab {
    Toolbox,
    Outline,
}

const DRAG_THRESHOLD: f32 = 3.0;
const MARGIN: f32 = 24.0;
const TITLE_HEIGHT: f32 = 31.0;
const ZOOMS: [f32; 7] = [0.5, 0.67, 0.75, 1.0, 1.25, 1.5, 2.0];

pub struct AxamlDesigner {
    /// The `.axaml` file's editor: the designer reads and writes its text.
    editor: Entity<EditorView>,
    pub path: PathBuf,
    /// The view as last read (kept while the XAML has an error, so typing doesn't blank it).
    doc: Option<Doc>,
    /// Why the current text can't be read.
    error: Option<String>,
    last_text: String,
    selection: Option<ElPath>,
    /// The toolbox entry the next click places.
    tool: Option<&'static str>,
    drag: Option<Drag>,
    /// Where the control being dragged would go.
    plan: Option<DropPlan>,
    /// The view while a resize is going on.
    preview: Option<Doc>,
    /// The page each TabControl shows.
    tabs: HashMap<ElPath, usize>,
    layout: Layout,
    /// Where the view's top left is in the window.
    origin: Rc<Cell<Option<Point<Pixels>>>>,
    zoom: f32,
    /// Dark (true) or light; `None`: as the view (or the app) asks, else as ByteForge's theme.
    dark: Option<bool>,
    left: LeftTab,
    focus: FocusHandle,
    input: Entity<TextInput>,
    editing: Option<Row>,
    dropdown: Option<Row>,
    show_events: bool,
    refocus: bool,
    message: Option<(String, bool)>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<AxamlEvent> for AxamlDesigner {}

impl Focusable for AxamlDesigner {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn rgba(c: u32) -> Hsla {
    gpui::rgba(c).into()
}

fn color(t: &Theme, key: &str, fallback: &str) -> Hsla {
    if t.has(key) { t.c(key) } else { t.c(fallback) }
}

/// Whether a file is an Avalonia view the designer can open.
pub fn is_axaml(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("axaml"))
}

/// The code-behind of a view (`MainWindow.axaml` → `MainWindow.axaml.cs`).
pub fn code_behind(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".cs");
    PathBuf::from(s)
}

impl AxamlDesigner {
    pub fn new(editor: Entity<EditorView>, path: PathBuf, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextInput::new("", cx));
        let subs = vec![
            cx.subscribe(&editor, |this: &mut Self, _, event: &EditorEvent, cx| {
                if matches!(event, EditorEvent::Edited) {
                    this.reload(cx);
                }
            }),
            cx.subscribe(&input, |this: &mut Self, _, event: &InputEvent, cx| match event {
                InputEvent::Confirmed => this.finish_edit(true, cx),
                InputEvent::Cancelled => this.finish_edit(false, cx),
                _ => {}
            }),
        ];
        let mut this = Self {
            editor,
            path,
            doc: None,
            error: None,
            last_text: String::new(),
            selection: None,
            tool: None,
            drag: None,
            plan: None,
            preview: None,
            tabs: HashMap::new(),
            layout: Layout::default(),
            origin: Rc::new(Cell::new(None)),
            zoom: 1.0,
            dark: None,
            left: LeftTab::Toolbox,
            focus: cx.focus_handle(),
            input,
            editing: None,
            dropdown: None,
            show_events: false,
            refocus: false,
            message: None,
            _subs: subs,
        };
        this.reload(cx);
        this
    }

    pub fn focus(&self, window: &mut Window) {
        window.focus(&self.focus);
    }

    #[cfg(test)]
    pub fn model(&self) -> Option<&Doc> {
        self.doc.as_ref()
    }

    #[cfg(test)]
    pub fn selection(&self) -> Option<&ElPath> {
        self.selection.as_ref()
    }

    #[cfg(test)]
    pub fn layout_rect(&self, path: &[usize]) -> Option<Rect> {
        self.layout.get(path).map(|p| p.rect)
    }

    #[cfg(test)]
    pub fn message(&self) -> Option<&str> {
        self.message.as_ref().map(|(m, _)| m.as_str())
    }

    #[cfg(test)]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The class of the code-behind.
    pub fn class_name(&self) -> Option<String> {
        self.doc.as_ref().and_then(|d| d.class_name()).map(str::to_owned)
    }

    /// Reads the file's text again when it isn't what the designer wrote (typed in the XAML, undone, reloaded).
    fn reload(&mut self, cx: &mut Context<Self>) {
        let text = self.editor.read(cx).state.buffer.text();
        if text == self.last_text && (self.doc.is_some() || self.error.is_some()) {
            return;
        }
        self.last_text = text;
        match Doc::parse(&self.last_text) {
            Ok(doc) => {
                if self.selection.as_ref().is_some_and(|s| doc.get(s).is_none()) {
                    self.selection = None;
                }
                self.doc = Some(doc);
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
        self.preview = None;
        self.drag = None;
        self.plan = None;
        cx.notify();
    }

    /// Writes new text into the file (one undo step) and selects `select`.
    fn commit(&mut self, text: String, select: Option<ElPath>, cx: &mut Context<Self>) {
        match Doc::parse(&text) {
            Ok(doc) => {
                self.selection = select.filter(|s| doc.get(s).is_some()).or_else(|| self.selection.clone().filter(|s| doc.get(s).is_some()));
                if let Some(s) = self.selection.clone() {
                    self.show_page_of(&doc, &s);
                }
                self.doc = Some(doc);
                self.error = None;
            }
            Err(e) => {
                self.message = Some((format!("That change would break the XAML: {e}"), true));
                cx.notify();
                return;
            }
        }
        self.preview = None;
        if text != self.last_text {
            self.last_text = text.clone();
            self.editor.update(cx, |e, cx| e.replace_text_minimal(&text, cx));
        }
        cx.notify();
    }

    /// Changes the view: `f` gets the current document and returns the new text (and what to select).
    fn change(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&Doc) -> Result<(String, Option<ElPath>), String>) {
        if self.error.is_some() {
            self.message = Some(("Fix the XAML first (F7: View Code); the designer reads it again as you type.".into(), true));
            cx.notify();
            return;
        }
        let Some(doc) = self.doc.clone() else { return };
        match f(&doc) {
            Ok((text, select)) => {
                self.message = None;
                self.commit(text, select, cx);
            }
            Err(e) => {
                self.message = Some((e, true));
                cx.notify();
            }
        }
    }

    fn select(&mut self, path: Option<ElPath>, cx: &mut Context<Self>) {
        self.finish_edit(true, cx);
        self.dropdown = None;
        if let (Some(doc), Some(p)) = (self.doc.clone(), &path) {
            self.show_page_of(&doc, p);
        }
        self.selection = path;
        cx.notify();
    }

    /// Selects the element written at a line of the XAML (the code's caret moved there).
    pub fn select_line(&mut self, line: usize, cx: &mut Context<Self>) {
        let Some(path) = self.doc.as_ref().and_then(|d| d.at_line(line)) else { return };
        if self.selection.as_ref() != Some(&path) && self.drag.is_none() && self.editing.is_none() {
            self.select(Some(path), cx);
        }
    }

    /// Shows the tab pages an element is on.
    fn show_page_of(&mut self, doc: &Doc, path: &[usize]) {
        for depth in 0..path.len() {
            let parent = &path[..depth];
            if doc.get(parent).is_some_and(|e| e.local() == "TabControl") {
                self.tabs.insert(parent.to_vec(), path[depth]);
            }
        }
    }

    fn selected(&self) -> ElPath {
        self.selection.clone().unwrap_or_default()
    }

    fn is_dark(&self, cx: &App) -> bool {
        if let Some(d) = self.dark {
            return d;
        }
        let asked = self.doc.as_ref().and_then(|d| d.root.attr("RequestedThemeVariant").map(str::to_owned)).or_else(|| app_theme_variant(&self.path));
        match asked.as_deref() {
            Some("Dark") => true,
            Some("Light") => false,
            _ => !Theme::global(cx).data.is_light(),
        }
    }

    // ───────────── actions ─────────────

    pub fn delete_selection(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.selection.clone().filter(|p| !p.is_empty()) else { return };
        let parent = path[..path.len() - 1].to_vec();
        self.change(cx, |d| Ok((d.remove(&[path])?, Some(parent))));
    }

    fn copy_selection(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(source) = self.selection.as_ref().filter(|p| !p.is_empty()).and_then(|p| self.doc.as_ref()?.source(p)) else { return false };
        cx.write_to_clipboard(ClipboardItem::new_string(source));
        true
    }

    fn cut_selection(&mut self, cx: &mut Context<Self>) {
        if self.copy_selection(cx) {
            self.delete_selection(cx);
        }
    }

    /// Where a control added without a place goes: into the selected container (at its end), else after
    /// the selected control.
    fn default_drop(&self, doc: &Doc) -> Option<ax::Drop> {
        let mut at = self.selected();
        loop {
            let el = doc.get(&at)?;
            let takes = match el.role() {
                Role::Panel | Role::Items => true,
                Role::Content | Role::Decorator => el.children.is_empty() && ax::content_text(el).is_none_or(|t| t.is_empty() || el.attr(el.info().map_or("Content", |c| c.content_prop)).is_some()),
                Role::Leaf => false,
            };
            if takes {
                return Some(ax::Drop { parent: at.clone(), index: el.children.len(), attrs: Vec::new(), in_place: false });
            }
            // A window (or border) holding a panel: into the panel.
            if matches!(el.role(), Role::Content | Role::Decorator)
                && el.children.len() == 1
                && matches!(el.children[0].role(), Role::Panel | Role::Items)
            {
                let mut inner = at.clone();
                inner.push(0);
                return Some(ax::Drop { parent: inner, index: el.children[0].children.len(), attrs: Vec::new(), in_place: false });
            }
            let (i, up) = at.split_last().map(|(i, p)| (*i, p.to_vec()))?;
            // After the selected control in its panel.
            if doc.get(&up).is_some_and(|p| matches!(p.role(), Role::Panel | Role::Items)) {
                return Some(ax::Drop { parent: up, index: i + 1, attrs: Vec::new(), in_place: false });
            }
            at = up;
        }
    }

    /// Ctrl+V: XAML from the clipboard into the selected container (or after the selected control).
    fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) else { return };
        let Some(doc) = &self.doc else { return };
        if !doc.accepts(&text) {
            self.message = Some(("The clipboard doesn't hold a XAML element.".into(), true));
            cx.notify();
            return;
        }
        let Some(drop) = self.default_drop(doc) else {
            self.message = Some(("There's no room for it here: select a panel first.".into(), true));
            cx.notify();
            return;
        };
        self.change(cx, |d| d.insert_at(&drop, &text).map(|(t, p)| (t, Some(p))));
    }

    /// Adds a control from the toolbox: where a drop plan says, else into the selected container.
    fn add(&mut self, tag: &'static str, plan: Option<ax::Drop>, cx: &mut Context<Self>) {
        let Some(doc) = &self.doc else { return };
        let Some(mut drop) = plan.or_else(|| self.default_drop(doc)) else {
            self.message = Some(("There's no room for it here: select a panel, or drop it where it should go.".into(), true));
            cx.notify();
            return;
        };
        let Some(info) = ax::control(tag) else { return };
        // On a Canvas nothing stretches it: it gets a size.
        if let Some((w, h)) = info.canvas_size
            && doc.get(&drop.parent).is_some_and(|p| p.local() == "Canvas")
            && !info.snippet.contains("Width=")
        {
            drop.attrs.push(("Width".into(), Some(ax::format_num(w))));
            drop.attrs.push(("Height".into(), Some(ax::format_num(h))));
        }
        let xaml = ax::indent_snippet(info.snippet, &doc.indent);
        // A content control holding text gets the control in its place.
        let parent = doc.get(&drop.parent);
        let holds_text = parent.is_some_and(|p| matches!(p.role(), Role::Content | Role::Decorator) && p.children.is_empty() && p.text.is_some());
        self.change(cx, |d| {
            if holds_text {
                let text = d.replace_text_with(&drop.parent, &xaml)?;
                let mut path = drop.parent.clone();
                path.push(0);
                let after = Doc::parse(&text)?;
                return Ok((if drop.attrs.is_empty() { text } else { after.set_attrs(&path, &drop.attrs)? }, Some(path)));
            }
            d.insert_at(&drop, &xaml).map(|(t, p)| (t, Some(p)))
        });
    }

    /// Goes to the handler of an element's event, hooking up a new one first if it has none.
    pub fn go_to_handler(&mut self, path: &[usize], event: &str, cx: &mut Context<Self>) {
        let Some(doc) = &self.doc else { return };
        let Some(el) = doc.get(path) else { return };
        let (args, namespace) = ax::event_args(event);
        let method = match el.attr(event).filter(|h| !ax::is_markup(h)) {
            Some(h) => h.to_owned(),
            None => {
                let base = el.name().map(str::to_owned).unwrap_or_else(|| el.local().to_owned());
                let used: Vec<String> = doc.all().iter().flat_map(|(_, e)| e.attrs.iter().map(|a| a.value.clone())).collect();
                let mut method = format!("{base}_On{event}");
                let mut n = 2;
                while used.contains(&method) {
                    method = format!("{base}{n}_On{event}");
                    n += 1;
                }
                let (p, ev, m) = (path.to_vec(), event.to_owned(), method.clone());
                self.change(cx, move |d| Ok((d.set_attr(&p, &ev, Some(&m))?, Some(p.clone()))));
                if self.message.is_some() {
                    return;
                }
                method
            }
        };
        cx.emit(AxamlEvent::Handler { method, args: args.to_owned(), namespace: namespace.to_owned() });
    }

    fn default_event(&mut self, path: &[usize], cx: &mut Context<Self>) {
        let Some(tag) = self.doc.as_ref().and_then(|d| d.get(path)).map(|e| e.tag.clone()) else { return };
        if self.class_name().is_none() {
            self.message = Some(("The view has no x:Class, so there's no code-behind for handlers.".into(), true));
            cx.notify();
            return;
        }
        self.go_to_handler(path, ax::default_event(&tag), cx);
    }

    fn view_code(&mut self, cx: &mut Context<Self>) {
        let line = self.selection.as_ref().and_then(|p| self.doc.as_ref()?.get(p)).map(|e| e.line);
        cx.emit(AxamlEvent::ViewCode { line });
    }

    /// Arrow keys: on a Canvas the control moves by pixels, in a Grid from cell to cell, in other panels
    /// before or after its neighbours.
    fn nudge(&mut self, dx: i32, dy: i32, big: bool, cx: &mut Context<Self>) {
        let Some(path) = self.selection.clone().filter(|p| !p.is_empty()) else { return };
        let Some(doc) = &self.doc else { return };
        let Some((parent, _)) = doc.parent(&path) else { return };
        let Some(el) = doc.get(&path) else { return };
        let num = |a: &str| el.attr(a).and_then(ax::parse_num).unwrap_or(0.0);
        let step = if big { 8.0 } else { 1.0 };
        let changes: Vec<(String, Option<String>)> = match parent.local() {
            "Canvas" => vec![
                ("Canvas.Left".into(), Some(ax::format_num(num("Canvas.Left") + dx as f32 * step))),
                ("Canvas.Top".into(), Some(ax::format_num(num("Canvas.Top") + dy as f32 * step))),
            ],
            "Grid" => {
                let cols = parent.definitions(false).len().max(1) as f32;
                let rows = parent.definitions(true).len().max(1) as f32;
                let c = (num("Grid.Column") + dx as f32).clamp(0.0, cols - 1.0);
                let r = (num("Grid.Row") + dy as f32).clamp(0.0, rows - 1.0);
                vec![("Grid.Column".into(), (c > 0.0).then(|| ax::format_num(c))), ("Grid.Row".into(), (r > 0.0).then(|| ax::format_num(r)))]
            }
            _ => {
                let i = *path.last().unwrap_or(&0);
                let to = if dx + dy < 0 { i.checked_sub(1) } else { (i + 1 < parent.children.len()).then_some(i + 2) };
                let Some(index) = to else { return };
                let drop = ax::Drop { parent: path[..path.len() - 1].to_vec(), index, attrs: Vec::new(), in_place: false };
                self.change(cx, |d| d.move_to(&path, &drop).map(|(t, p)| (t, Some(p))));
                return;
            }
        };
        let changes: Vec<_> = changes.into_iter().filter(|(n, v)| v.is_some() || el.attr(n).is_some()).collect();
        self.change(cx, |d| Ok((d.set_attrs(&path, &changes)?, Some(path.clone()))));
    }

    // ───────────── the property grid ─────────────

    fn rows(doc: &Doc, path: &[usize]) -> Vec<(&'static str, Vec<Row>)> {
        let Some(el) = doc.get(path) else { return Vec::new() };
        let parent = doc.parent(path).map(|(p, _)| p.tag.clone());
        let known = ax::properties(&el.tag, parent.as_deref());
        let events = ax::events(&el.tag);
        let mut cats: Vec<(&'static str, Vec<Row>)> = vec![("Design", vec![Row::Name])];
        for p in &known {
            match cats.iter_mut().find(|(c, _)| *c == p.category) {
                Some((_, rows)) => rows.push(Row::Known(p)),
                None => cats.push((p.category, vec![Row::Known(p)])),
            }
        }
        let others: Vec<Row> = el
            .attrs
            .iter()
            .filter(|a| {
                let n = a.name.as_str();
                n != "x:Name" && n != "Name" && n != "x:Class" && n != "mc:Ignorable" && !n.starts_with("xmlns") && !known.iter().any(|p| p.name == n) && !events.contains(&n)
            })
            .map(|a| Row::Other(a.name.clone()))
            .collect();
        if !others.is_empty() {
            cats.push(("Other", others));
        }
        let order = ["Design", "Common", "Layout", "Brush", "Appearance", "Text", "Other"];
        cats.sort_by_key(|(c, _)| order.iter().position(|o| o == c).unwrap_or(order.len()));
        cats
    }

    fn click_value(&mut self, row: Row, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.as_ref() == Some(&row) {
            return;
        }
        self.finish_edit(true, cx);
        if let Row::Known(p) = &row
            && matches!(p.kind, Kind::Bool | Kind::Enum(_) | Kind::Brush)
        {
            self.dropdown = if self.dropdown.as_ref() == Some(&row) { None } else { Some(row) };
            cx.notify();
            return;
        }
        self.start_edit(row, window, cx);
    }

    fn start_edit(&mut self, row: Row, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = &self.doc else { return };
        let path = self.selected();
        let text = doc.get(&path).and_then(|e| if row == Row::Name { e.name().map(str::to_owned) } else { e.value(&row.attr()) }).unwrap_or_default();
        self.input = cx.new(|cx| {
            let mut i = TextInput::new("", cx);
            i.set_text(text, cx);
            i.select_all_text(cx);
            i
        });
        self._subs.truncate(1);
        self._subs.push(cx.subscribe(&self.input, |this: &mut Self, _, event: &InputEvent, cx| match event {
            InputEvent::Confirmed => this.finish_edit(true, cx),
            InputEvent::Cancelled => this.finish_edit(false, cx),
            _ => {}
        }));
        self.dropdown = None;
        self.editing = Some(row);
        window.focus(&self.input.focus_handle(cx));
        cx.notify();
    }

    fn finish_edit(&mut self, apply: bool, cx: &mut Context<Self>) {
        let Some(row) = self.editing.take() else { return };
        self.refocus = true;
        cx.notify();
        if !apply {
            return;
        }
        let value = self.input.read(cx).text().to_owned();
        self.set_value(&row, &value, cx);
    }

    /// Sets a property from the grid (an empty value removes it).
    fn set_value(&mut self, row: &Row, value: &str, cx: &mut Context<Self>) {
        let path = self.selected();
        let el = self.doc.as_ref().and_then(|d| d.get(&path));
        let attr = match row {
            // A name written as Name="…" stays that way.
            Row::Name => if el.is_some_and(|e| e.attr("Name").is_some() && e.attr("x:Name").is_none()) { "Name".to_owned() } else { "x:Name".to_owned() },
            _ => row.attr(),
        };
        let trimmed = if matches!(row, Row::Known(Prop { kind: Kind::Text, .. }) | Row::Other(_)) { value.to_owned() } else { value.trim().to_owned() };
        if let Row::Name = row
            && !trimmed.is_empty()
            && !trimmed.chars().all(|c| c.is_alphanumeric() || c == '_')
        {
            self.message = Some((format!("\"{trimmed}\" isn't a valid name."), true));
            cx.notify();
            return;
        }
        let current = el.and_then(|e| e.value(&attr));
        let new = (!trimmed.is_empty()).then_some(trimmed);
        if current == new {
            return;
        }
        // A design size needs the d: namespace (with mc:Ignorable) on the root.
        let needs_d = attr.starts_with("d:") && self.doc.as_ref().is_some_and(|d| !d.has_prefix("d"));
        self.change(cx, |d| {
            let declared = if needs_d { Doc::parse(&add_design_namespace(&d.text)?)? } else { d.clone() };
            Ok((declared.set_attr(&path, &attr, new.as_deref())?, Some(path.clone())))
        });
    }

    // ───────────── the mouse ─────────────

    fn to_view(&self, p: Point<Pixels>) -> Option<(f32, f32)> {
        let o = self.origin.get()?;
        Some((f32::from(p.x - o.x) / self.zoom, f32::from(p.y - o.y) / self.zoom))
    }

    fn handle_rect(rect: Rect, h: Handle, size: f32) -> Rect {
        let cx = rect.x + (h.0 as f32 + 1.0) * rect.w / 2.0;
        let cy = rect.y + (h.1 as f32 + 1.0) * rect.h / 2.0;
        Rect::new(cx - size / 2.0, cy - size / 2.0, size, size)
    }

    /// The selection's resize handles: (handle, its square, the selection's rectangle), in view pixels.
    fn handles(&self, size: f32) -> Vec<(Handle, Rect, Rect)> {
        let path = self.selected();
        let Some(p) = self.layout.get(&path) else { return Vec::new() };
        let set: &[Handle] = if path.is_empty() { &ROOT_HANDLES } else { &ALL_HANDLES };
        set.iter().map(|&h| (h, Self::handle_rect(p.rect, h, size), p.rect)).collect()
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        self.finish_edit(true, cx);
        self.dropdown = None;
        let (Some(doc), Some(p)) = (self.doc.clone(), self.to_view(event.position)) else { return };
        if let Some(tag) = self.tool {
            self.drag = Some(Drag { kind: DragKind::Create { tag }, start: p, now: p, moved: false });
            self.plan = self.layout.drop_at(&doc, p.0, p.1, None, tag);
            cx.notify();
            return;
        }
        let hs = (Theme::global(cx).n("formDesigner.handleSize", 7.0) + 2.0) / self.zoom;
        if let Some((handle, _, rect)) = self.handles(hs).into_iter().find(|(_, r, _)| r.contains(p.0, p.1)) {
            self.drag = Some(Drag { kind: DragKind::Resize { path: self.selected(), handle, rect }, start: p, now: p, moved: false });
            return;
        }
        let Some(hit) = self.layout.hit(p.0, p.1).cloned() else {
            self.select(Some(Vec::new()), cx);
            return;
        };
        // A tab header shows its page.
        if let Some((parent, _)) = doc.parent(&hit.path)
            && parent.local() == "TabControl"
        {
            self.tabs.insert(hit.path[..hit.path.len() - 1].to_vec(), *hit.path.last().unwrap_or(&0));
        }
        self.select(Some(hit.path.clone()), cx);
        if event.click_count >= 2 {
            self.default_event(&hit.path, cx);
            return;
        }
        if !hit.path.is_empty() {
            let grab = (p.0 - hit.rect.x, p.1 - hit.rect.y);
            self.drag = Some(Drag { kind: DragKind::Move { path: hit.path, grab, rect: hit.rect }, start: p, now: p, moved: false });
        }
        cx.notify();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.to_view(event.position) else { return };
        let Some(drag) = &mut self.drag else { return };
        if event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        drag.now = p;
        if !drag.moved && ((p.0 - drag.start.0).abs() > DRAG_THRESHOLD || (p.1 - drag.start.1).abs() > DRAG_THRESHOLD) {
            drag.moved = true;
        }
        if !drag.moved {
            return;
        }
        let Some(doc) = self.doc.clone() else { return };
        match &drag.kind {
            DragKind::Move { path, grab, .. } => {
                let tag = doc.get(path).map(|e| e.tag.clone()).unwrap_or_default();
                self.plan = self.layout.drop_at(&doc, p.0, p.1, Some((path, *grab)), &tag);
            }
            DragKind::Create { tag } => self.plan = self.layout.drop_at(&doc, p.0, p.1, None, tag),
            DragKind::Resize { path, handle, rect } => {
                let r = resized(*rect, *handle, p.0 - drag.start.0, p.1 - drag.start.1);
                let changes = resize_changes(&doc, path, *rect, r);
                self.preview = doc.set_attrs(path, &changes).ok().and_then(|t| Doc::parse(&t).ok());
            }
        }
        cx.notify();
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(mut drag) = self.drag.take() else { return };
        let plan = self.plan.take();
        if let Some(p) = self.to_view(event.position) {
            drag.now = p;
        }
        let Some(doc) = self.doc.clone() else { return };
        match drag.kind {
            DragKind::Create { tag } => {
                if !event.modifiers.shift {
                    self.tool = None;
                }
                match plan {
                    Some(plan) => self.add(tag, Some(plan.drop), cx),
                    None => {
                        self.message = Some((format!("A {tag} can't go there: drop it into a panel (or an empty content control)."), true));
                        cx.notify();
                    }
                }
            }
            DragKind::Move { path, .. } if drag.moved => match plan {
                Some(plan) => self.change(cx, |d| d.move_to(&path, &plan.drop).map(|(t, p)| (t, Some(p)))),
                None => {
                    self.message = Some(("It can't go there: drop it into a panel (or an empty content control).".into(), true));
                    cx.notify();
                }
            },
            DragKind::Resize { path, handle, rect } if drag.moved => {
                let r = resized(rect, handle, drag.now.0 - drag.start.0, drag.now.1 - drag.start.1);
                let changes = resize_changes(&doc, &path, rect, r);
                let needs_d = path.is_empty() && changes.iter().any(|(n, _)| n.starts_with("d:")) && !doc.has_prefix("d");
                self.change(cx, |d| {
                    let declared = if needs_d { Doc::parse(&add_design_namespace(&d.text)?)? } else { d.clone() };
                    Ok((declared.set_attrs(&path, &changes)?, Some(path.clone())))
                });
            }
            _ => {}
        }
        self.preview = None;
        cx.notify();
    }

    fn on_drop_control(&mut self, tool: &DraggedControl, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.clone() else { return };
        let Some(p) = self.to_view(window.mouse_position()) else { return };
        self.tool = None;
        match self.layout.drop_at(&doc, p.0, p.1, None, tool.0) {
            Some(plan) => self.add(tool.0, Some(plan.drop), cx),
            None => {
                self.message = Some((format!("A {} can't go there: drop it into a panel (or an empty content control).", tool.0), true));
                cx.notify();
            }
        }
        window.focus(&self.focus);
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.input.focus_handle(cx).is_focused(window) || self.doc.is_none() {
            return;
        }
        let key = &event.keystroke;
        let m = key.modifiers;
        let secondary = m.secondary() && !m.alt;
        let arrow = match key.key.as_str() {
            "left" => Some((-1, 0)),
            "right" => Some((1, 0)),
            "up" => Some((0, -1)),
            "down" => Some((0, 1)),
            _ => None,
        };
        if let Some((dx, dy)) = arrow {
            self.nudge(dx, dy, secondary, cx);
            cx.stop_propagation();
            return;
        }
        match key.key.as_str() {
            "delete" | "backspace" if !m.modified() => {
                self.delete_selection(cx);
                cx.stop_propagation();
            }
            "escape" => {
                if self.tool.take().is_none()
                    && let Some(path) = self.selection.clone()
                    && let Some((_, up)) = path.split_last()
                {
                    self.select(Some(up.to_vec()), cx);
                }
                cx.notify();
                cx.stop_propagation();
            }
            "z" if secondary => {
                self.editor.update(cx, |e, cx| e.undo_redo(m.shift, cx));
                cx.stop_propagation();
            }
            "y" if secondary => {
                self.editor.update(cx, |e, cx| e.undo_redo(true, cx));
                cx.stop_propagation();
            }
            "c" if secondary && !m.shift => {
                self.copy_selection(cx);
                cx.stop_propagation();
            }
            "x" if secondary && !m.shift => {
                self.cut_selection(cx);
                cx.stop_propagation();
            }
            "v" if secondary && !m.shift => {
                self.paste(cx);
                cx.stop_propagation();
            }
            "enter" if !m.modified() => {
                let path = self.selected();
                self.default_event(&path, cx);
                cx.stop_propagation();
            }
            _ => {}
        }
    }
}

/// `rect` with the side(s) of `handle` dragged by (dx, dy).
fn resized(rect: Rect, handle: Handle, dx: f32, dy: f32) -> Rect {
    let mut r = rect;
    match handle.0 {
        -1 => {
            let dx = dx.min(rect.w - 1.0);
            r.x += dx;
            r.w -= dx;
        }
        1 => r.w = (rect.w + dx).max(1.0),
        _ => {}
    }
    match handle.1 {
        -1 => {
            let dy = dy.min(rect.h - 1.0);
            r.y += dy;
            r.h -= dy;
        }
        1 => r.h = (rect.h + dy).max(1.0),
        _ => {}
    }
    r
}

/// The attributes a resize sets: Width and Height (the root's design size, unless it sets its own size),
/// and on a Canvas the position when the left or top edge moved.
fn resize_changes(doc: &Doc, path: &[usize], from: Rect, to: Rect) -> Vec<(String, Option<String>)> {
    let Some(el) = doc.get(path) else { return Vec::new() };
    let mut out = Vec::new();
    let (w_attr, h_attr) = if path.is_empty() {
        (if el.attr("Width").is_some() { "Width" } else { "d:DesignWidth" }, if el.attr("Height").is_some() { "Height" } else { "d:DesignHeight" })
    } else {
        ("Width", "Height")
    };
    if (to.w - from.w).abs() >= 0.5 {
        out.push((w_attr.to_owned(), Some(ax::format_num(to.w.round()))));
    }
    if (to.h - from.h).abs() >= 0.5 {
        out.push((h_attr.to_owned(), Some(ax::format_num(to.h.round()))));
    }
    if let Some((parent, _)) = doc.parent(path)
        && parent.local() == "Canvas"
    {
        let num = |a: &str| el.attr(a).and_then(ax::parse_num).unwrap_or(0.0);
        if (to.x - from.x).abs() >= 0.5 {
            out.push(("Canvas.Left".into(), Some(ax::format_num((num("Canvas.Left") + to.x - from.x).round()))));
        }
        if (to.y - from.y).abs() >= 0.5 {
            out.push(("Canvas.Top".into(), Some(ax::format_num((num("Canvas.Top") + to.y - from.y).round()))));
        }
    }
    out
}

/// Declares the d: namespace (and mc:Ignorable="d") on the root, so design sizes can be set.
fn add_design_namespace(text: &str) -> Result<String, String> {
    let doc = Doc::parse(text)?;
    let mut changes = vec![("xmlns:d".to_owned(), Some("http://schemas.microsoft.com/expression/blend/2008".to_owned()))];
    if !doc.has_prefix("mc") {
        changes.push(("xmlns:mc".into(), Some("http://schemas.openxmlformats.org/markup-compatibility/2006".into())));
    }
    if doc.root.attr("mc:Ignorable").is_none() {
        changes.push(("mc:Ignorable".into(), Some("d".into())));
    }
    // The namespaces go in before the attributes that use them.
    let tag_end = doc.root.range.start + 1 + doc.root.tag.len();
    let sep = if doc.text[tag_end..doc.root.start_tag_end].contains('\n') {
        let line = doc.root.attrs.first().map(|a| a.range.start).unwrap_or(tag_end);
        let ls = doc.text[..line].rfind('\n').map_or(0, |p| p + 1);
        format!("\n{}", &doc.text[ls..line])
    } else {
        " ".to_owned()
    };
    let insert: String = changes.iter().map(|(n, v)| format!("{sep}{n}=\"{}\"", v.as_deref().unwrap_or_default())).collect();
    let mut out = text.to_owned();
    out.insert_str(doc.root.attrs.last().map_or(tag_end, |a| a.range.end), &insert);
    Ok(out)
}

/// The theme the app asks for in its App.axaml (looked for next to the view and in the folders above it).
fn app_theme_variant(view: &Path) -> Option<String> {
    let mut dir = view.parent();
    for _ in 0..4 {
        let d = dir?;
        if let Ok(text) = std::fs::read_to_string(d.join("App.axaml")) {
            return Doc::parse(&text).ok().and_then(|doc| doc.root.attr("RequestedThemeVariant").map(str::to_owned));
        }
        if std::fs::read_dir(d).ok()?.flatten().any(|e| e.path().extension().is_some_and(|x| x == "csproj")) {
            return None;
        }
        dir = d.parent();
    }
    None
}

/// The project folder of a view (the nearest folder above it with a .csproj), else the view's folder.
fn project_dir(view: &Path) -> PathBuf {
    let mut dir = view.parent();
    while let Some(d) = dir {
        if std::fs::read_dir(d).is_ok_and(|mut r| r.any(|e| e.is_ok_and(|e| e.path().extension().is_some_and(|x| x == "csproj")))) {
            return d.to_path_buf();
        }
        dir = d.parent();
    }
    view.parent().map(Path::to_path_buf).unwrap_or_default()
}

// ───────────── drawing ─────────────

/// Fluent's colors, light or dark.
#[derive(Clone, Copy)]
struct Palette {
    window: u32,
    text: u32,
    muted: u32,
    control: u32,
    control_border: u32,
    field: u32,
    field_border: u32,
    accent: u32,
    on_accent: u32,
    track: u32,
    selected_item: u32,
    header: u32,
}

const LIGHT: Palette = Palette {
    window: 0xffffffff,
    text: 0x1b1b1bff,
    muted: 0x6b6b6bff,
    control: 0xe6e6e6ff,
    control_border: 0xd0d0d0ff,
    field: 0xffffffff,
    field_border: 0x8a8a8aff,
    accent: 0x0078d4ff,
    on_accent: 0xffffffff,
    track: 0xc8c8c8ff,
    selected_item: 0xe0eefaff,
    header: 0xf3f3f3ff,
};

const DARK: Palette = Palette {
    window: 0x202020ff,
    text: 0xffffffff,
    muted: 0xa0a0a0ff,
    control: 0x3a3a3aff,
    control_border: 0x4a4a4aff,
    field: 0x2b2b2bff,
    field_border: 0x8a8a8aff,
    accent: 0x4cc2ffff,
    on_accent: 0x000000ff,
    track: 0x5a5a5aff,
    selected_item: 0x1f3d57ff,
    header: 0x2b2b2bff,
};

struct Look<'a> {
    doc: &'a Doc,
    pal: Palette,
    z: f32,
    font_family: SharedString,
    project: PathBuf,
    view_dir: PathBuf,
}

impl Look<'_> {
    fn brush(&self, el: &Element, attr: &str) -> Option<Hsla> {
        el.attr(attr).and_then(ax::parse_color).map(rgba)
    }

    fn fg(&self, p: &Placed) -> Hsla {
        rgba(p.font.fg.unwrap_or(self.pal.text))
    }

    fn text(&self, s: impl Into<SharedString>, p: &Placed, color: Hsla) -> gpui::Div {
        self.text_sized(s, p.font.size, p.font.bold, p.font.italic, color)
    }

    fn text_sized(&self, s: impl Into<SharedString>, size: f32, bold: bool, italic: bool, color: Hsla) -> gpui::Div {
        let z = self.z;
        div()
            .text_size(px(size * z))
            .line_height(px((size * 1.33).round() * z))
            .font_family(self.font_family.clone())
            .text_color(color)
            .when(bold, |d| d.font_weight(gpui::FontWeight::SEMIBOLD))
            .when(italic, |d| d.italic())
            .whitespace_nowrap()
            .child(s.into())
    }

    fn px(&self, v: f32) -> Pixels {
        px(v * self.z)
    }

    /// A thickness scaled ([l, t, r, b]) as padding on a div.
    fn pad(&self, d: gpui::Div, t: [f32; 4]) -> gpui::Div {
        d.pl(self.px(t[0])).pt(self.px(t[1])).pr(self.px(t[2])).pb(self.px(t[3]))
    }

    fn corner(&self, el: &Element, default: f32) -> Pixels {
        self.px(el.attr("CornerRadius").and_then(ax::parse_thickness).map_or(default, |c| c[0]))
    }

    /// The text inside a control's content area, aligned as its content alignment says.
    fn content(&self, el: &Element, p: &Placed, d: gpui::Div, centered: bool, fg: Hsla) -> gpui::Div {
        let d = d.flex().overflow_hidden();
        let h = el.attr("HorizontalContentAlignment").unwrap_or(if centered { "Center" } else { "Left" });
        let v = el.attr("VerticalContentAlignment").unwrap_or("Center");
        let d = match h {
            "Center" => d.justify_center(),
            "Right" => d.justify_end(),
            _ => d.justify_start(),
        };
        let d = match v {
            "Top" => d.items_start(),
            "Bottom" => d.items_end(),
            _ => d.items_center(),
        };
        match (el.children.is_empty(), ax::content_text(el)) {
            (true, Some(t)) => d.child(self.text(strip_access_key(&t), p, fg)),
            _ => d,
        }
    }

    fn element(&self, p: &Placed, tabs: &HashMap<ElPath, usize>) -> AnyElement {
        let Some(el) = self.doc.get(&p.path) else { return div().into_any_element() };
        let pal = self.pal;
        let fg = self.fg(p);
        let enabled = el.attr("IsEnabled").and_then(ax::parse_bool) != Some(false);
        let fg = if enabled { fg } else { rgba(pal.muted) };
        let bg = self.brush(el, "Background");
        let chrome = {
            let pd = ax::padding(el);
            let b = ax::border(el);
            [pd[0] + b[0], pd[1] + b[1], pd[2] + b[2], pd[3] + b[3]]
        };
        let outline = |d: gpui::Div| d.border_1().border_dashed().border_color(rgba(0x80808066));
        let checked = el.attr("IsChecked").and_then(ax::parse_bool) == Some(true);
        let tag = el.local();
        let base = div().size_full().relative();
        match tag {
            "TextBlock" | "SelectableTextBlock" => {
                let t = ax::content_text(el).unwrap_or_default();
                let wrap = el.attr("TextWrapping").is_some_and(|w| w != "NoWrap");
                let align = el.attr("TextAlignment").unwrap_or("Left");
                let text = self.text(t, p, fg).when(wrap, |d| d.whitespace_normal()).when(align == "Center", |d| d.text_center()).when(align == "Right", |d| d.text_right());
                self.pad(base.when_some(bg, |d, c| d.bg(c)), chrome).overflow_hidden().child(text).into_any_element()
            }
            "Button" | "ToggleButton" | "RepeatButton" => {
                let on = tag == "ToggleButton" && checked;
                let back = bg.unwrap_or(rgba(if on { pal.accent } else { pal.control }));
                let text_color = if on && el.attr("Foreground").is_none() { rgba(pal.on_accent) } else { fg };
                let d = self.pad(base.bg(back).rounded(self.corner(el, 4.0)).border_1().border_color(self.brush(el, "BorderBrush").unwrap_or(rgba(pal.control_border))), chrome);
                self.content(el, p, d, true, text_color).into_any_element()
            }
            "HyperlinkButton" => {
                let d = self.pad(base, chrome);
                self.content(el, p, d, false, rgba(pal.accent)).underline().into_any_element()
            }
            "Label" | "ContentControl" | "ListBoxItem" | "ComboBoxItem" => {
                let selected = el.attr("IsSelected").and_then(ax::parse_bool) == Some(true) || self.selected_in_list(p);
                let back = bg.or(selected.then(|| rgba(pal.selected_item)));
                let d = self.pad(base.when_some(back, |d, c| d.bg(c)).rounded(self.px(4.0)), chrome);
                self.content(el, p, d, false, fg).into_any_element()
            }
            "TextBox" | "MaskedTextBox" | "NumericUpDown" => {
                let value = el.value("Text").map(|v| ax::display_text(&v));
                let value = if tag == "NumericUpDown" { Some(el.attr("Value").unwrap_or("0").to_owned()) } else { value };
                let password = el.attr("PasswordChar").is_some_and(|c| !c.is_empty());
                let (shown, color) = match value.filter(|v| !v.is_empty()) {
                    Some(v) if password => ("●".repeat(v.chars().count()), fg),
                    Some(v) => (v, fg),
                    None => (el.attr("Watermark").map(ax::display_text).unwrap_or_default(), rgba(pal.muted)),
                };
                let multiline = el.attr("AcceptsReturn").and_then(ax::parse_bool) == Some(true) || el.attr("TextWrapping").is_some_and(|w| w != "NoWrap");
                let field = base
                    .bg(bg.unwrap_or(rgba(pal.field)))
                    .rounded(self.corner(el, 4.0))
                    .border_1()
                    .border_b_2()
                    .border_color(self.brush(el, "BorderBrush").unwrap_or(rgba(pal.field_border)))
                    .overflow_hidden()
                    .flex();
                let text = self.pad(div().flex_1().min_w_0().h_full().flex().when(!multiline, |d| d.items_center()), chrome).child(self.text(shown, p, color).when(multiline, |d| d.whitespace_normal()));
                field
                    .child(text)
                    .when(tag == "NumericUpDown", |d| d.child(div().flex_none().w(self.px(32.)).h_full().flex().flex_col().items_center().justify_center().text_size(self.px(9.)).text_color(fg).child("▲").child("▼")))
                    .into_any_element()
            }
            "CheckBox" | "RadioButton" => {
                let round = tag == "RadioButton";
                let mark = div()
                    .flex_none()
                    .size(self.px(20.))
                    .mr(self.px(8.))
                    .bg(rgba(if checked { pal.accent } else { pal.field }))
                    .border_1()
                    .border_color(rgba(if checked { pal.accent } else { pal.field_border }))
                    .when(round, |d| d.rounded_full())
                    .when(!round, |d| d.rounded(self.px(4.)))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(checked && round, |d| d.child(div().size(self.px(8.)).rounded_full().bg(rgba(pal.on_accent))))
                    .when(checked && !round, |d| d.child(div().text_size(self.px(13.)).text_color(rgba(pal.on_accent)).child("✓")));
                let label = ax::content_text(el).filter(|_| el.children.is_empty()).map(|t| self.text(strip_access_key(&t), p, fg));
                base.flex().items_center().when_some(bg, |d, c| d.bg(c)).child(mark).children(label).into_any_element()
            }
            "ToggleSwitch" => {
                let state = if checked { el.attr("OnContent").unwrap_or("On") } else { el.attr("OffContent").unwrap_or("Off") };
                let header = ax::content_text(el).filter(|t| !t.is_empty() && el.children.is_empty());
                let knob = div().size(self.px(12.)).rounded_full().bg(rgba(if checked { pal.on_accent } else { pal.text }));
                let pill = div()
                    .flex_none()
                    .w(self.px(40.))
                    .h(self.px(20.))
                    .rounded_full()
                    .px(self.px(4.))
                    .flex()
                    .items_center()
                    .when(checked, |d| d.bg(rgba(pal.accent)).justify_end())
                    .when(!checked, |d| d.border_1().border_color(rgba(pal.field_border)))
                    .child(knob);
                base.flex()
                    .flex_col()
                    .justify_end()
                    .children(header.map(|h| self.text(h, p, fg)))
                    .child(div().h(self.px(32.)).flex().items_center().gap(self.px(12.)).child(pill).child(self.text(state.to_owned(), p, fg)))
                    .into_any_element()
            }
            "ComboBox" => {
                let index = el.attr("SelectedIndex").and_then(ax::parse_num).map(|n| n as i64).unwrap_or(-1);
                let selected = (index >= 0).then(|| el.children.get(index as usize)).flatten().or_else(|| el.children.iter().find(|c| c.attr("IsSelected").and_then(ax::parse_bool) == Some(true)));
                let (shown, color) = match selected.and_then(ax::content_text) {
                    Some(t) => (t, fg),
                    None => (el.attr("PlaceholderText").map(ax::display_text).unwrap_or_default(), rgba(pal.muted)),
                };
                base.bg(bg.unwrap_or(rgba(pal.field)))
                    .rounded(self.corner(el, 4.0))
                    .border_1()
                    .border_color(self.brush(el, "BorderBrush").unwrap_or(rgba(pal.field_border)))
                    .flex()
                    .items_center()
                    .child(div().flex_1().min_w_0().pl(self.px(12.)).overflow_hidden().child(self.text(shown, p, color)))
                    .child(div().flex_none().w(self.px(32.)).flex().justify_center().text_size(self.px(12.)).text_color(fg).child("⌄"))
                    .into_any_element()
            }
            "DatePicker" | "TimePicker" => {
                let parts: &[&str] = if tag == "DatePicker" { &["month", "day", "year"] } else { &["hour", "minute", "AM"] };
                base.bg(bg.unwrap_or(rgba(pal.field)))
                    .rounded(self.px(4.))
                    .border_1()
                    .border_color(rgba(pal.field_border))
                    .flex()
                    .children(parts.iter().enumerate().map(|(i, part)| {
                        div().flex_1().h_full().flex().items_center().justify_center().when(i > 0, |d| d.border_l_1().border_color(rgba(pal.control_border))).child(self.text(*part, p, rgba(pal.muted)))
                    }))
                    .into_any_element()
            }
            "Calendar" => {
                let small = Placed { font: ax::Font { size: 12.0, ..p.font }, ..p.clone() };
                let days = ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"];
                base.bg(bg.unwrap_or(rgba(pal.field)))
                    .rounded(self.px(4.))
                    .border_1()
                    .border_color(rgba(pal.control_border))
                    .p(self.px(8.))
                    .flex()
                    .flex_col()
                    .gap(self.px(6.))
                    .child(self.text("Month Year", p, fg).font_weight(gpui::FontWeight::SEMIBOLD))
                    .child(div().flex().children(days.iter().map(|d| div().flex_1().flex().justify_center().child(self.text(*d, &small, rgba(pal.muted))))))
                    .children((0..6).map(|row| div().flex().children((0..7).map(move |col| row * 7 + col - 2).map(|n| div().flex_1().flex().justify_center().when((1..=30).contains(&n), |d| d.child(self.text(n.to_string(), &small, fg)))))))
                    .into_any_element()
            }
            "Slider" => {
                let num = |a: &str, d: f32| el.attr(a).and_then(ax::parse_num).unwrap_or(d);
                let (min, max, value) = (num("Minimum", 0.0), num("Maximum", 100.0), num("Value", 0.0));
                let frac = ((value - min) / (max - min).max(0.0001)).clamp(0.0, 1.0);
                let w = p.rect.w;
                let thumb = 20.0;
                let at = 2.0 + frac * (w - thumb - 4.0).max(0.0);
                let mid = p.rect.h / 2.0;
                base.child(div().absolute().left(self.px(2.)).right(self.px(2.)).top(self.px(mid - 2.)).h(self.px(4.)).rounded_full().bg(rgba(pal.track)))
                    .child(div().absolute().left(self.px(2.)).w(self.px(at + thumb / 2.0 - 2.0)).top(self.px(mid - 2.)).h(self.px(4.)).rounded_full().bg(rgba(pal.accent)))
                    .child(div().absolute().left(self.px(at)).top(self.px(mid - thumb / 2.0)).size(self.px(thumb)).rounded_full().bg(rgba(pal.field)).border_1().border_color(rgba(pal.control_border)).flex().items_center().justify_center().child(div().size(self.px(10.)).rounded_full().bg(rgba(pal.accent))))
                    .into_any_element()
            }
            "ProgressBar" => {
                let num = |a: &str, d: f32| el.attr(a).and_then(ax::parse_num).unwrap_or(d);
                let (min, max, value) = (num("Minimum", 0.0), num("Maximum", 100.0), num("Value", 0.0));
                let indeterminate = el.attr("IsIndeterminate").and_then(ax::parse_bool) == Some(true);
                let frac = if indeterminate { 0.4 } else { ((value - min) / (max - min).max(0.0001)).clamp(0.0, 1.0) };
                let bar = self.brush(el, "Foreground").unwrap_or(rgba(pal.accent));
                base.flex()
                    .items_center()
                    .child(div().relative().w_full().h(self.px(p.rect.h.clamp(1.0, 4.0))).rounded_full().bg(rgba(pal.track)).child(
                        div().absolute().top_0().bottom_0().when(indeterminate, |d| d.left(gpui::relative(0.3))).w(gpui::relative(frac)).rounded_full().bg(bar),
                    ))
                    .into_any_element()
            }
            "Separator" => base.flex().items_center().child(div().w_full().h(self.px(1.)).bg(rgba(pal.control_border))).into_any_element(),
            "Image" => {
                let source = el.attr("Source").filter(|s| !ax::is_markup(s)).and_then(|s| self.resolve(s));
                let stretch = el.attr("Stretch").unwrap_or("Uniform");
                let fit = match stretch {
                    "Fill" => gpui::ObjectFit::Fill,
                    "UniformToFill" => gpui::ObjectFit::Cover,
                    "None" => gpui::ObjectFit::None,
                    _ => gpui::ObjectFit::Contain,
                };
                match source {
                    Some(path) => base.child(gpui::img(path).size_full().object_fit(fit)).into_any_element(),
                    None => outline(base).flex().items_center().justify_center().child(self.text("🖼", p, rgba(pal.muted))).into_any_element(),
                }
            }
            "Rectangle" | "Ellipse" => {
                let fill = self.brush(el, "Fill");
                let stroke = self.brush(el, "Stroke");
                let st = el.attr("StrokeThickness").and_then(ax::parse_num).unwrap_or(1.0);
                let radius = el.attr("RadiusX").and_then(ax::parse_num).unwrap_or(0.0);
                let d = base.when_some(fill, |d, c| d.bg(c)).when(tag == "Ellipse", |d| d.rounded_full()).when(tag == "Rectangle", |d| d.rounded(self.px(radius)));
                let d = match stroke {
                    Some(c) => d.border(self.px(st)).border_color(c),
                    None if fill.is_none() => outline(d),
                    None => d,
                };
                d.into_any_element()
            }
            "Border" => {
                let b = el.attr("BorderThickness").and_then(ax::parse_thickness).unwrap_or([0.0; 4]);
                let brush = self.brush(el, "BorderBrush");
                let d = base.when_some(bg, |d, c| d.bg(c)).rounded(self.corner(el, 0.0));
                let d = match brush {
                    Some(c) if b.iter().any(|v| *v > 0.0) => d.border_l(self.px(b[0])).border_t(self.px(b[1])).border_r(self.px(b[2])).border_b(self.px(b[3])).border_color(c),
                    _ if bg.is_none() => outline(d),
                    _ => d,
                };
                d.into_any_element()
            }
            "ScrollViewer" => base.when_some(bg, |d, c| d.bg(c)).into_any_element(),
            "Expander" => {
                let expanded = el.attr("IsExpanded").and_then(ax::parse_bool) == Some(true);
                let header = el.attr("Header").map(ax::display_text).unwrap_or_default();
                base.flex()
                    .flex_col()
                    .child(
                        div()
                            .flex_none()
                            .h(self.px(48.))
                            .px(self.px(16.))
                            .flex()
                            .items_center()
                            .justify_between()
                            .bg(rgba(pal.header))
                            .border_1()
                            .border_color(rgba(pal.control_border))
                            .rounded(self.px(4.))
                            .child(self.text(header, p, fg))
                            .child(self.text(if expanded { "⌃" } else { "⌄" }, p, fg)),
                    )
                    .when(expanded, |d| d.child(div().flex_1().bg(bg.unwrap_or(rgba(pal.window))).border_1().border_t_0().border_color(rgba(pal.control_border))))
                    .into_any_element()
            }
            "TabControl" => base.when_some(bg, |d, c| d.bg(c)).into_any_element(),
            "TabItem" => {
                let header = el.attr("Header").map(ax::display_text).unwrap_or_default();
                let shown = self.shown_tab(&p.path, tabs);
                let big = Placed { font: ax::Font { size: el.attr("FontSize").and_then(ax::parse_num).unwrap_or(20.0), ..p.font }, ..p.clone() };
                base.flex()
                    .items_center()
                    .px(self.px(12.))
                    .child(self.text(strip_access_key(&header), &big, if shown { fg } else { rgba(pal.muted) }))
                    .when(shown, |d| d.child(div().absolute().left(self.px(12.)).right(self.px(12.)).bottom(self.px(4.)).h(self.px(3.)).rounded_full().bg(rgba(pal.accent))))
                    .into_any_element()
            }
            "ListBox" => base.bg(bg.unwrap_or(rgba(pal.field))).border_1().border_color(self.brush(el, "BorderBrush").unwrap_or(rgba(pal.control_border))).into_any_element(),
            "Menu" => base.when_some(bg, |d, c| d.bg(c)).into_any_element(),
            "MenuItem" => {
                let header = el.attr("Header").map(ax::display_text).unwrap_or_default();
                self.pad(base.flex().items_center(), chrome).child(self.text(strip_access_key(&header), p, fg)).into_any_element()
            }
            "Window" | "UserControl" => base.into_any_element(),
            _ => match el.role() {
                // Panels: their background, else a faint outline so an empty one can be found; a Grid shows
                // its rows and columns.
                Role::Panel => {
                    let mut d = match bg {
                        Some(c) => base.bg(c),
                        None => outline(base),
                    };
                    if tag == "Grid" {
                        d = d.children(self.grid_lines(&p.path, p.rect, rgba(0x80808059)));
                    }
                    d.into_any_element()
                }
                _ => {
                    // A control the designer doesn't know: its name in a box.
                    let known = el.info().is_some();
                    let d = if known { base.when_some(bg, |d, c| d.bg(c)) } else { outline(base).flex().items_center().justify_center() };
                    if known {
                        let d = self.pad(d, chrome);
                        self.content(el, p, d, false, fg).into_any_element()
                    } else {
                        d.child(self.text(el.tag.clone(), p, rgba(pal.muted))).into_any_element()
                    }
                }
            },
        }
    }

    /// The dashed lines between a Grid's rows and columns.
    fn grid_lines(&self, path: &[usize], rect: Rect, line: Hsla) -> Vec<AnyElement> {
        let mut out = Vec::new();
        if let Some(edges) = GRID_EDGES.with(|g| g.borrow().get(path).cloned()) {
            for x in edges.0.iter().skip(1).take(edges.0.len().saturating_sub(2)) {
                out.push(div().absolute().top_0().bottom_0().left(self.px(x - rect.x)).w(px(1.)).border_l_1().border_dashed().border_color(line).into_any_element());
            }
            for y in edges.1.iter().skip(1).take(edges.1.len().saturating_sub(2)) {
                out.push(div().absolute().left_0().right_0().top(self.px(y - rect.y)).h(px(1.)).border_t_1().border_dashed().border_color(line).into_any_element());
            }
        }
        out
    }

    /// Whether a list item is the one its ListBox's SelectedIndex picks.
    fn selected_in_list(&self, p: &Placed) -> bool {
        let Some((parent, _)) = self.doc.parent(&p.path) else { return false };
        parent.local() == "ListBox" && parent.attr("SelectedIndex").and_then(ax::parse_num).is_some_and(|n| n as i64 == *p.path.last().unwrap_or(&0) as i64)
    }

    fn shown_tab(&self, path: &[usize], tabs: &HashMap<ElPath, usize>) -> bool {
        let Some((parent, up)) = self.doc.parent(path) else { return false };
        let selected = parent.children.iter().position(|c| c.attr("IsSelected").and_then(ax::parse_bool) == Some(true));
        let shown = tabs.get(up).copied().or(selected).or_else(|| parent.attr("SelectedIndex").and_then(ax::parse_num).map(|n| n.max(0.0) as usize)).unwrap_or(0);
        path.last() == Some(&shown)
    }

    /// A file an image's Source names: `avares://Assembly/Assets/x.png` and `/Assets/x.png` are in the
    /// project; other paths are next to the view.
    fn resolve(&self, source: &str) -> Option<PathBuf> {
        let rel = if let Some(rest) = source.strip_prefix("avares://") {
            rest.split_once('/').map(|(_, p)| self.project.join(p))
        } else if let Some(rest) = source.strip_prefix('/') {
            Some(self.project.join(rest))
        } else if source.contains("://") {
            None
        } else {
            Some(self.view_dir.join(source))
        };
        rel.filter(|p| p.is_file())
    }
}

/// The x of each column edge and the y of each row edge of a Grid.
type GridEdges = (Vec<f32>, Vec<f32>);

thread_local! {
    /// The edges of each Grid's tracks as last laid out (for drawing its lines).
    static GRID_EDGES: std::cell::RefCell<HashMap<ElPath, GridEdges>> = std::cell::RefCell::new(HashMap::new());
}

/// `_File` → `File` (the underscore marks the access key).
fn strip_access_key(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    let mut done = false;
    while let Some(c) = chars.next() {
        if c == '_' && !done {
            if chars.peek() == Some(&'_') {
                out.push('_');
                chars.next();
            } else {
                done = true;
            }
            continue;
        }
        out.push(c);
    }
    out
}

impl AxamlDesigner {
    fn render_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::global(cx);
        let Some(doc) = self.preview.clone().or_else(|| self.doc.clone()) else {
            let error = self.error.clone().unwrap_or_default();
            return div()
                .p_4()
                .flex()
                .flex_col()
                .gap_2()
                .text_color(color(&t, "formDesigner.foreground", "sideBar.foreground"))
                .child(div().child(format!("The designer can't read {}.", self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())))
                .child(div().text_color(t.c("foreground.muted")).child(error))
                .child(div().child("Fix the XAML (F7: View Code); the designer reads it again as you type."))
                .into_any_element();
        };
        // Text is measured with the font it is drawn with.
        let text_system = window.text_system().clone();
        let family = t.ui_font.clone();
        let measure = move |s: &str, size: f32, bold: bool| -> f32 {
            if s.is_empty() {
                return 0.0;
            }
            let font = gpui::Font { family: family.clone(), weight: if bold { gpui::FontWeight::SEMIBOLD } else { gpui::FontWeight::NORMAL }, ..gpui::font(family.clone()) };
            let run = gpui::TextRun { len: s.len(), font, color: gpui::black(), background_color: None, underline: None, strikethrough: None };
            f32::from(text_system.shape_line(SharedString::from(s.to_owned()), px(size), &[run], None).width)
        };
        self.layout = Layout::of(&doc, &measure, &self.tabs);
        GRID_EDGES.with(|g| {
            let mut g = g.borrow_mut();
            g.clear();
            for a in &self.layout.areas {
                if let ax::AreaKind::Grid { cols, rows } = &a.kind {
                    g.insert(a.path.clone(), (cols.clone(), rows.clone()));
                }
            }
        });
        let z = self.zoom;
        let dark = self.is_dark(cx);
        let pal = if dark { DARK } else { LIGHT };
        let look = Look { doc: &doc, pal, z, font_family: t.ui_font.clone(), project: project_dir(&self.path), view_dir: self.path.parent().map(Path::to_path_buf).unwrap_or_default() };
        let (w, h) = self.layout.size;
        let root_bg = doc.root.attr("Background").and_then(ax::parse_color).unwrap_or(pal.window);
        let selection_color = color(&t, "formDesigner.selectionBorder", "focusBorder");
        let handle_bg = t.c_or("formDesigner.handleBackground", 0xffffffff);
        let handle_border = t.c_or("formDesigner.handleBorder", 0x000000ff);
        let hs = t.n("formDesigner.handleSize", 7.0);
        let r = |r: Rect| div().absolute().left(px(r.x * z)).top(px(r.y * z)).w(px(r.w * z)).h(px(r.h * z));

        let controls: Vec<AnyElement> = self
            .layout
            .items
            .iter()
            .skip(1)
            .filter(|p| p.clip.w > 0.0 && p.clip.h > 0.0)
            .map(|p| {
                let inner = look.element(p, &self.tabs);
                let opacity = doc.get(&p.path).and_then(|e| e.attr("Opacity")).and_then(ax::parse_num).unwrap_or(1.0).clamp(0.0, 1.0);
                r(p.clip).overflow_hidden().when(opacity < 1.0, |d| d.opacity(opacity.max(0.15))).child(
                    div().absolute().left(px((p.rect.x - p.clip.x) * z)).top(px((p.rect.y - p.clip.y) * z)).w(px(p.rect.w * z)).h(px(p.rect.h * z)).child(inner),
                ).into_any_element()
            })
            .collect();
        let origin = self.origin.clone();
        let accent = color(&t, "formDesigner.dropTargetBorder", "focusBorder");
        let selected = self.selection.clone();
        let client = div()
            .id("axaml-view")
            .debug_selector(|| "axaml-view".into())
            .relative()
            .w(px(w * z))
            .h(px(h * z))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .overflow_hidden()
                    .bg(rgba(root_bg))
                    .font_family(t.ui_font.clone())
                    .children(controls)
                    .child(gpui::canvas(move |bounds, _, _| origin.set(Some(bounds.origin)), |_, _, _, _| {}).absolute().inset_0()),
            )
            .when_some(selected.as_ref().filter(|s| !s.is_empty()).and_then(|s| self.layout.get(s)), |d, p| {
                let rr = p.rect;
                d.child(div().absolute().left(px(rr.x * z - 1.)).top(px(rr.y * z - 1.)).w(px(rr.w * z + 2.)).h(px(rr.h * z + 2.)).border_1().border_dashed().border_color(selection_color))
            })
            .children(self.handles(hs / z).into_iter().map(|(_, hr, _)| r(hr).bg(handle_bg).border_1().border_color(handle_border)))
            .when_some(self.plan.as_ref().filter(|_| self.drag.as_ref().is_some_and(|d| d.moved || matches!(d.kind, DragKind::Create { .. }))), |d, plan| {
                d.child(r(plan.area).border_2().border_color(accent)).children(plan.line.map(|l| r(l).bg(accent)))
            })
            .when_some(self.drag.as_ref().filter(|d| d.moved), |d, drag| match &drag.kind {
                DragKind::Move { grab, rect, .. } => d.child(r(Rect::new(drag.now.0 - grab.0, drag.now.1 - grab.1, rect.w, rect.h)).border_1().border_dashed().border_color(selection_color).bg(gpui::hsla(0.58, 0.8, 0.5, 0.08))),
                _ => d,
            });
        let root_selected = selected.as_ref().is_some_and(|s| s.is_empty());
        let window_root = doc.root.local() == "Window";
        let title = doc.root.attr("Title").map(ax::display_text).unwrap_or_default();
        let title_bar = window_root.then(|| {
            div()
                .flex()
                .items_center()
                .h(px(TITLE_HEIGHT))
                .pl(px(10.))
                .bg(rgba(if dark { 0x2b2b2bff } else { 0xf3f3f3ff }))
                .text_color(rgba(pal.text))
                .text_size(px(12.))
                .font_family(t.ui_font.clone())
                .child(div().size(px(14.)).mr(px(8.)).rounded(px(3.)).bg(rgba(0x8b44acff)))
                .child(div().flex_1().min_w_0().whitespace_nowrap().overflow_hidden().child(title))
                .children(["—", "☐", "✕"].map(|g| div().w(px(46.)).flex().justify_center().child(g)))
        });
        div()
            .p(px(MARGIN))
            .flex()
            .flex_col()
            .items_start()
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .when(window_root, |d| d.border_1().border_color(t.c_or("formDesigner.windowBorder", 0x7a7a7aff)).shadow(t.shadow("widget.shadow")))
                    .when(root_selected, |d| d.border_1().border_dashed().border_color(selection_color))
                    .children(title_bar)
                    .child(client),
            )
            .into_any_element()
    }

    fn render_toolbox(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::global(cx);
        let fg = color(&t, "formDesigner.foreground", "sideBar.foreground");
        let muted = t.c("foreground.muted");
        let hover = t.c("list.hoverBackground");
        let active = t.c("list.activeSelectionBackground");
        let tool = self.tool;
        let entry = |id: SharedString, label: &'static str, selected: bool| {
            div().id(id.clone()).debug_selector(move || id.to_string()).px_3().py(px(3.)).rounded(px(3.)).cursor_pointer().text_color(fg).when(selected, |d| d.bg(active)).hover(move |s| s.bg(hover)).child(label)
        };
        div()
            .child(entry("axaml-tool-pointer".into(), "Pointer", tool.is_none()).on_click(cx.listener(|this, _, _, cx| {
                this.tool = None;
                cx.notify();
            })))
            .children(ax::toolbox().into_iter().map(|(group, items)| {
                div().child(div().px_2().pt_2().pb(px(2.)).text_size(px(11.)).text_color(muted).child(group)).children(items.into_iter().map(|c| {
                    let name = c.name;
                    entry(format!("axaml-tool-{name}").into(), name, tool == Some(name))
                        .on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                            if e.click_count() >= 2 {
                                this.tool = None;
                                this.add(name, None, cx);
                            } else {
                                this.tool = if this.tool == Some(name) { None } else { Some(name) };
                            }
                            cx.notify();
                        }))
                        .on_drag(DraggedControl(name), |d, _, _, cx| cx.new(|_| d.clone()))
                }))
            }))
            .into_any_element()
    }

    fn render_outline(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::global(cx);
        let fg = color(&t, "formDesigner.foreground", "sideBar.foreground");
        let hover = t.c("list.hoverBackground");
        let active = t.c("list.activeSelectionBackground");
        let muted = t.c("foreground.muted");
        let Some(doc) = &self.doc else { return div().into_any_element() };
        div()
            .children(doc.all().into_iter().map(|(path, el)| {
                let selected = self.selection.as_ref() == Some(&path);
                let id = format!("axaml-outline-{}", path.iter().map(usize::to_string).collect::<Vec<_>>().join("-"));
                let hidden = el.attr("IsVisible").and_then(ax::parse_bool) == Some(false);
                let depth = path.len();
                let p = path.clone();
                div()
                    .id(SharedString::from(id.clone()))
                    .debug_selector(move || id.clone())
                    .pl(px(8. + 12. * depth as f32))
                    .pr_2()
                    .py(px(2.))
                    .rounded(px(3.))
                    .cursor_pointer()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_color(if hidden { muted } else { fg })
                    .when(selected, |d| d.bg(active))
                    .hover(move |s| s.bg(hover))
                    .child(el.label())
                    .on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                        this.select(Some(p.clone()), cx);
                        if e.click_count() >= 2 {
                            this.view_code(cx);
                        }
                    }))
            }))
            .into_any_element()
    }

    fn render_left(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::global(cx);
        let fg = color(&t, "formDesigner.foreground", "sideBar.foreground");
        let muted = t.c("foreground.muted");
        let line = color(&t, "formDesigner.border", "editorWidget.border");
        let tab = |id: &'static str, label: &'static str, on: bool| {
            div().id(id).debug_selector(move || id.into()).px_2().py(px(2.)).cursor_pointer().text_color(if on { fg } else { muted }).when(on, |d| d.border_b_2().border_color(t.c("focusBorder"))).child(label)
        };
        let body = match self.left {
            LeftTab::Toolbox => self.render_toolbox(cx),
            LeftTab::Outline => self.render_outline(cx),
        };
        div()
            .flex_none()
            .w(t.m("formDesigner.toolboxWidth", 170.0) + px(30.))
            .h_full()
            .flex()
            .flex_col()
            .bg(t.bg("sideBar.background"))
            .border_r_1()
            .border_color(line)
            .text_size(t.m("ui.size", 13.0))
            .child(
                div()
                    .flex()
                    .gap_1()
                    .px_1()
                    .border_b_1()
                    .border_color(line)
                    .child(tab("axaml-left-toolbox", "Toolbox", self.left == LeftTab::Toolbox).on_click(cx.listener(|this, _, _, cx| {
                        this.left = LeftTab::Toolbox;
                        cx.notify();
                    })))
                    .child(tab("axaml-left-outline", "Outline", self.left == LeftTab::Outline).on_click(cx.listener(|this, _, _, cx| {
                        this.left = LeftTab::Outline;
                        cx.notify();
                    }))),
            )
            .child(div().id("axaml-left-body").flex_1().min_h_0().overflow_y_scroll().p_1().child(body))
    }

    fn render_grid(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::global(cx);
        let fg = color(&t, "formDesigner.foreground", "sideBar.foreground");
        let muted = t.c("foreground.muted");
        let hover = t.c("list.hoverBackground");
        let line = color(&t, "formDesigner.border", "editorWidget.border");
        let row_h = t.m("formDesigner.rowHeight", 22.0);
        let label_w = px(130.);
        let mut body = div().flex().flex_col();
        let doc = self.doc.clone();
        let path = self.selected();
        let el = doc.as_ref().and_then(|d| d.get(&path)).cloned();
        let header = match &el {
            Some(el) => div()
                .px_2()
                .py_1()
                .border_b_1()
                .border_color(line)
                .child(div().text_color(fg).child(el.name().map_or_else(|| format!("({})", el.local()), str::to_owned)))
                .child(div().text_size(px(11.)).text_color(muted).child(el.tag.clone())),
            None => div(),
        };
        if let (Some(doc), Some(el)) = (&doc, &el) {
            let tab = |id: &'static str, label: &'static str, on: bool| {
                div().id(id).debug_selector(move || id.into()).px_2().py(px(2.)).cursor_pointer().text_color(if on { fg } else { muted }).when(on, |d| d.border_b_2().border_color(t.c("focusBorder"))).child(label)
            };
            body = body.child(
                div()
                    .flex()
                    .gap_1()
                    .px_1()
                    .border_b_1()
                    .border_color(line)
                    .child(tab("axaml-tab-properties", "Properties", !self.show_events).on_click(cx.listener(|this, _, _, cx| {
                        this.show_events = false;
                        cx.notify();
                    })))
                    .child(tab("axaml-tab-events", "Events", self.show_events).on_click(cx.listener(|this, _, _, cx| {
                        this.show_events = true;
                        cx.notify();
                    }))),
            );
            if self.show_events {
                for &event in ax::events(&el.tag) {
                    let handler = el.attr(event).map(str::to_owned);
                    let (p, unhook) = (path.clone(), path.clone());
                    body = body.child(
                        div()
                            .id(SharedString::from(format!("axaml-event-{event}")))
                            .debug_selector(move || format!("axaml-event-{event}"))
                            .h(row_h)
                            .flex()
                            .items_center()
                            .border_b_1()
                            .border_color(line)
                            .cursor_pointer()
                            .hover(move |s| s.bg(hover))
                            .child(div().flex_none().w(label_w).px_2().text_color(fg).whitespace_nowrap().overflow_hidden().child(event))
                            .child(div().flex_1().min_w_0().px_2().whitespace_nowrap().overflow_hidden().text_color(if handler.is_some() { fg } else { muted }).child(handler.clone().unwrap_or_else(|| "(double-click to add)".into())))
                            .when(handler.is_some(), |d| {
                                d.child(div().id(SharedString::from(format!("axaml-unhook-{event}"))).px_1().text_color(muted).cursor_pointer().child("✕").tooltip(crate::workbench::tooltip("Unhook (keeps the method)")).on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        let p = unhook.clone();
                                        this.change(cx, |d| Ok((d.set_attr(&p, event, None)?, Some(p.clone()))));
                                    },
                                )))
                            })
                            .on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                                let hooked = this.doc.as_ref().and_then(|d| d.get(&p)).is_some_and(|el| el.attr(event).is_some());
                                if e.click_count() >= 2 || hooked {
                                    if this.class_name().is_none() {
                                        this.message = Some(("The view has no x:Class, so there's no code-behind for handlers.".into(), true));
                                        cx.notify();
                                        return;
                                    }
                                    this.go_to_handler(&p, event, cx);
                                }
                            })),
                    );
                }
            } else {
                for (category, rows) in Self::rows(doc, &path) {
                    body = body.child(div().px_2().pt_2().pb(px(2.)).text_size(px(11.)).text_color(muted).child(category));
                    for row in rows {
                        let value = if row == Row::Name { el.name().map(str::to_owned) } else { el.value(&row.attr()) };
                        let set = value.is_some();
                        let label = row.label();
                        let editing = self.editing.as_ref() == Some(&row);
                        let id = format!("axaml-prop-{}", label.trim_matches(['(', ')']));
                        let rr = row.clone();
                        let value_el: AnyElement = if editing {
                            div().flex_1().min_w_0().child(self.input.clone()).into_any_element()
                        } else {
                            let swatch = matches!(&row, Row::Known(p) if p.kind == Kind::Brush).then(|| value.as_deref().and_then(ax::parse_color)).flatten();
                            let shown = value.clone().unwrap_or_else(|| default_text(el, &row));
                            div()
                                .flex_1()
                                .min_w_0()
                                .px_2()
                                .flex()
                                .items_center()
                                .gap_1()
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_color(if set { fg } else { muted })
                                .when(set, |d| d.font_weight(gpui::FontWeight::BOLD))
                                .when_some(swatch, |d, c| d.child(div().flex_none().size(px(12.)).border_1().border_color(line).bg(rgba(c))))
                                .child(shown.replace('\n', " "))
                                .into_any_element()
                        };
                        body = body.child(
                            div()
                                .id(SharedString::from(id.clone()))
                                .debug_selector(move || id.clone())
                                .min_h(row_h)
                                .flex()
                                .items_center()
                                .border_b_1()
                                .border_color(line)
                                .cursor_pointer()
                                .hover(move |s| s.bg(hover))
                                .child(div().flex_none().w(label_w).px_2().text_color(fg).whitespace_nowrap().overflow_hidden().child(label))
                                .child(value_el)
                                .on_click(cx.listener(move |this, _, w, cx| this.click_value(rr.clone(), w, cx))),
                        );
                        if self.dropdown.as_ref() == Some(&row)
                            && let Row::Known(p) = row
                        {
                            body = body.child(self.render_dropdown(p, cx));
                        }
                    }
                }
            }
        }
        let message = self.message.clone().map(|(m, error)| div().p_2().border_t_1().border_color(line).text_color(if error { t.c_or("errorForeground", 0xf14c4cff) } else { muted }).child(m));
        div()
            .flex_none()
            .w(t.m("formDesigner.propertiesWidth", 290.0))
            .h_full()
            .flex()
            .flex_col()
            .bg(t.bg("sideBar.background"))
            .border_l_1()
            .border_color(line)
            .text_size(t.m("ui.size", 13.0))
            .child(header)
            .child(div().id("axaml-grid").flex_1().min_h_0().overflow_y_scroll().child(body))
            .children(message)
    }

    fn render_dropdown(&self, p: &'static Prop, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::global(cx);
        let fg = color(&t, "formDesigner.foreground", "sideBar.foreground");
        let hover = t.c("list.hoverBackground");
        let line = color(&t, "formDesigner.border", "editorWidget.border");
        let row = Row::Known(p);
        let option = |id: String, label: String, value: String| {
            let row = row.clone();
            div()
                .id(SharedString::from(id.clone()))
                .debug_selector(move || id.clone())
                .pl(px(138.))
                .py(px(2.))
                .cursor_pointer()
                .text_color(fg)
                .hover(move |s| s.bg(hover))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.dropdown = None;
                    this.set_value(&row, &value, cx);
                }))
        };
        let list = div().py_1().bg(t.bg("menu.background")).border_b_1().border_color(line);
        let default = option("axaml-option-default".into(), "(default)".into(), String::new());
        match p.kind {
            Kind::Bool => list.child(option("axaml-option-True".into(), "True".into(), "True".into())).child(option("axaml-option-False".into(), "False".into(), "False".into())).child(default).into_any_element(),
            Kind::Enum(members) => list.children(members.iter().map(|m| option(format!("axaml-option-{m}"), (*m).to_owned(), (*m).to_owned()))).child(default).into_any_element(),
            Kind::Brush => {
                let swatches = ax::BRUSHES.iter().map(|name| {
                    let c = ax::parse_color(name).unwrap_or(0);
                    let id = format!("axaml-color-{name}");
                    let value = (*name).to_owned();
                    let row = row.clone();
                    div()
                        .id(SharedString::from(id.clone()))
                        .debug_selector(move || id.clone())
                        .size(px(18.))
                        .border_1()
                        .border_color(line)
                        .bg(rgba(c))
                        .cursor_pointer()
                        .tooltip(crate::workbench::tooltip(*name))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.dropdown = None;
                            this.set_value(&row, &value, cx);
                        }))
                });
                let custom = row.clone();
                list.child(div().pl(px(138.)).pr_2().flex().flex_wrap().gap_1().children(swatches))
                    .child(default)
                    .child(div().id("axaml-color-custom").pl(px(138.)).py(px(2.)).cursor_pointer().text_color(fg).hover(move |s| s.bg(hover)).child("Custom… (#RRGGBB, a name or a resource)").on_click(cx.listener(move |this, _, w, cx| this.start_edit(custom.clone(), w, cx))))
                    .into_any_element()
            }
            _ => div().into_any_element(),
        }
    }
}

/// What a property shows when it isn't set: the default Avalonia uses where the designer knows it.
fn default_text(el: &Element, row: &Row) -> String {
    let Row::Known(p) = row else { return String::new() };
    match p.name {
        "HorizontalAlignment" => ax::alignment(el, true),
        "VerticalAlignment" => ax::alignment(el, false),
        "IsVisible" | "IsEnabled" | "CanResize" | "ShowInTaskbar" | "IsHitTestVisible" => "True".into(),
        "Opacity" => "1".into(),
        "Orientation" if el.local() == "StackPanel" => "Vertical".into(),
        "Orientation" => "Horizontal".into(),
        "LastChildFill" => "True".into(),
        "FontSize" => "14".into(),
        _ => match p.kind {
            Kind::Bool => "False".into(),
            Kind::Number if matches!(p.name, "Width" | "Height" | "MaxWidth" | "MaxHeight") => "Auto".into(),
            _ => String::new(),
        },
    }
}

impl Render for AxamlDesigner {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.refocus) && self.editing.is_none() && self.input.focus_handle(cx).is_focused(window) {
            window.focus(&self.focus);
        }
        let t = Theme::global(cx);
        let fg = t.c("breadcrumb.foreground");
        let muted = t.c("foreground.muted");
        let hover = t.c("toolbar.hoverBackground");
        let active = t.c("list.activeSelectionBackground");
        let has_doc = self.doc.is_some();
        let dark = self.is_dark(cx);
        let selection_is_control = self.selection.as_ref().is_some_and(|s| !s.is_empty());
        let button = |id: &'static str, label: SharedString| div().id(id).debug_selector(move || id.into()).px(px(6.)).py(px(1.)).rounded(px(4.)).cursor_pointer().hover(move |s| s.bg(hover)).child(label);
        let choice = |id: &'static str, label: &'static str, on: bool| button(id, label.into()).when(on, |d| d.bg(active));
        let name = self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let size = self.layout.size;
        let zoom = self.zoom;
        let toolbar = div()
            .flex_none()
            .h(t.m("breadcrumb.height", 22.0) + px(4.))
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .bg(t.bg("breadcrumb.background"))
            .border_b_1()
            .border_color(t.c("breadcrumb.border"))
            .text_size(t.m("breadcrumb.fontSize", 12.0))
            .text_color(fg)
            .child(div().whitespace_nowrap().child(format!("{name} [Design]")))
            .when(has_doc, |d| d.child(div().text_color(muted).whitespace_nowrap().child(format!("{} × {}", size.0.round(), size.1.round()))))
            .when_some(self.tool, |d, tool| d.child(div().text_color(muted).whitespace_nowrap().child(format!("Click or drag in the view to place a {tool} (Esc: cancel)"))))
            .child(div().flex_1())
            .child(choice("axaml-light", "Light", !dark).on_click(cx.listener(|this, _, _, cx| {
                this.dark = Some(false);
                cx.notify();
            })))
            .child(choice("axaml-dark", "Dark", dark).on_click(cx.listener(|this, _, _, cx| {
                this.dark = Some(true);
                cx.notify();
            })))
            .child(button("axaml-zoom-out", "−".into()).on_click(cx.listener(|this, _, _, cx| {
                this.zoom = ZOOMS.iter().rev().copied().find(|z| *z < this.zoom - 0.001).unwrap_or(ZOOMS[0]);
                cx.notify();
            })))
            .child(button("axaml-zoom-reset", format!("{}%", (zoom * 100.0).round()).into()).on_click(cx.listener(|this, _, _, cx| {
                this.zoom = 1.0;
                cx.notify();
            })))
            .child(button("axaml-zoom-in", "+".into()).on_click(cx.listener(|this, _, _, cx| {
                this.zoom = ZOOMS.iter().copied().find(|z| *z > this.zoom + 0.001).unwrap_or(ZOOMS[ZOOMS.len() - 1]);
                cx.notify();
            })))
            .when(selection_is_control, |d| d.child(button("axaml-delete", "Delete".into()).on_click(cx.listener(|this, _, _, cx| this.delete_selection(cx)))))
            .child(button("axaml-view-code", "View Code".into()).on_click(cx.listener(|this, _, _, cx| this.view_code(cx))));
        let error_bar = self.error.clone().filter(|_| has_doc).map(|e| {
            div().flex_none().px_2().py_1().bg(t.c_or("inputValidation.errorBackground", 0x5a1d1dff)).text_color(t.c_or("errorForeground", 0xf14c4cff)).text_size(px(12.)).child(format!("The XAML has an error; showing it as it last was. {e}"))
        });
        let view = self.render_view(window, cx);
        let scroller = div()
            .id("axaml-canvas")
            .debug_selector(|| "axaml-canvas".into())
            .absolute()
            .inset_0()
            .overflow_scroll()
            .bg(color(&t, "formDesigner.background", "editor.background"))
            .cursor(if self.tool.is_some() { gpui::CursorStyle::Crosshair } else { gpui::CursorStyle::Arrow })
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_drop(cx.listener(Self::on_drop_control))
            .child(view);
        let canvas = div().relative().flex_1().min_w_0().h_full().child(scroller);
        div()
            .key_context("FormDesigner")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .font_family(t.ui_font.clone())
            .on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(|this, _: &ViewCode, _, cx| this.view_code(cx)))
            .on_action(cx.listener(|this, _: &Undo, _, cx| this.editor.update(cx, |e, cx| e.undo_redo(false, cx))))
            .on_action(cx.listener(|this, _: &Redo, _, cx| this.editor.update(cx, |e, cx| e.undo_redo(true, cx))))
            .on_action(cx.listener(|this, _: &Copy, _, cx| {
                this.copy_selection(cx);
            }))
            .on_action(cx.listener(|this, _: &Cut, _, cx| this.cut_selection(cx)))
            .on_action(cx.listener(|this, _: &Paste, _, cx| this.paste(cx)))
            .child(toolbar)
            .children(error_bar)
            .child(div().flex_1().min_h_0().flex().when(has_doc, |d| d.child(self.render_left(cx))).child(canvas).when(has_doc, |d| d.child(self.render_grid(cx))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_keys_and_handles() {
        assert_eq!(strip_access_key("_File"), "File");
        assert_eq!(strip_access_key("Save __as"), "Save _as");
        let r = Rect::new(10.0, 10.0, 100.0, 50.0);
        assert_eq!(resized(r, Handle(1, 1), 5.0, 7.0), Rect::new(10.0, 10.0, 105.0, 57.0));
        assert_eq!(resized(r, Handle(-1, -1), 5.0, 7.0), Rect::new(15.0, 17.0, 95.0, 43.0));
        assert_eq!(code_behind(Path::new("/a/MainWindow.axaml")), PathBuf::from("/a/MainWindow.axaml.cs"));
    }

    #[test]
    fn adds_the_design_namespace() {
        let text = "<UserControl xmlns=\"https://github.com/avaloniaui\"\n             Width=\"10\">\n</UserControl>";
        let out = add_design_namespace(text).unwrap();
        assert!(out.contains("Width=\"10\"\n             xmlns:d=\"http://schemas.microsoft.com/expression/blend/2008\""), "{out}");
        assert!(out.contains("mc:Ignorable=\"d\""), "{out}");
        assert!(Doc::parse(&out).is_ok());
    }
}
