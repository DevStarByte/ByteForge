//! The Windows Forms designer: a form (or user control) drawn from its `.Designer.cs`, with a toolbox, the
//! property grid and the component tray. Controls are placed, moved, resized and dropped into containers
//! with the mouse (or nudged with the arrow keys); every change is written into `InitializeComponent`
//! through the file's own editor, so saving, undo and the tab's modified mark work as for any file.
//!
//! The form is drawn by ByteForge itself (not by Windows Forms), so the designer works the same on Linux
//! and macOS. Controls look like their Windows 11 counterparts closely enough to lay out a form; the
//! designer's own parts (canvas, toolbox, grid, selection) follow the theme.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use bf_core::winforms::{self as wf, Designer, Layout, Placed, PropInfo, PropKind, Rect};
use gpui::{
    AnyElement, App, ClickEvent, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable, Hsla, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, SharedString, Subscription, Window, div, prelude::*, px,
};

use crate::actions::{Copy, Cut, Paste, Redo, SelectAll, Undo, ViewCode};
use crate::editor::{EditorEvent, EditorView};
use crate::text_input::{InputEvent, TextInput};
use crate::theme::Theme;

mod localized;
mod strips;
use strips::ItemHit;

pub enum DesignerEvent {
    /// F7: the form's code.
    ViewCode,
    /// Go to (or first create) an event handler in the code file.
    Handler { method: String, args: String },
}

/// A toolbox entry being dragged onto the form.
#[derive(Clone)]
pub struct DraggedTool(pub &'static str);

impl Render for DraggedTool {
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
    Move { names: Vec<String> },
    Resize { name: String, handle: Handle, rect: Rect },
    Create { type_name: &'static str },
}

struct Drag {
    kind: DragKind,
    start: (i32, i32),
    now: (i32, i32),
    moved: bool,
    /// Snap to snap lines (not while Alt is held).
    snap: bool,
}

/// Controls copied in a designer (any designer: they paste into other forms too), with the code that was
/// put on the system clipboard for them; when the clipboard holds something else, there is nothing to paste.
struct DesignerClipboard {
    code: String,
    fragment: wf::Fragment,
}

impl gpui::Global for DesignerClipboard {}

/// How near (in pixels) an edge must come to a snap line to snap to it.
const SNAP_THRESHOLD: i32 = 6;

/// A row of the property grid.
#[derive(Clone)]
enum Row {
    /// (Name): renames the component.
    Name,
    Known(&'static PropInfo),
    /// A property the code sets that the grid doesn't know: edited as code.
    Other(String),
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

impl Row {
    fn label(&self) -> String {
        match self {
            Row::Name => "(Name)".into(),
            Row::Known(p) => p.name.into(),
            Row::Other(n) => n.clone(),
        }
    }
}

/// Where a new control goes: its location in its container, its size (`None`: the default), the
/// container and the part of it.
struct Placement {
    at: (i32, i32),
    size: Option<(i32, i32)>,
    owner: String,
    slot: String,
}

/// How far a mouse press must move before it is a drag.
const DRAG_THRESHOLD: i32 = 3;
/// Space around the form on the canvas.
const MARGIN: f32 = 24.0;
const TITLE_HEIGHT: f32 = 31.0;

pub struct FormDesigner {
    /// The `.Designer.cs` file's editor: the designer reads and writes its text.
    editor: Entity<EditorView>,
    pub code_path: PathBuf,
    pub designer_path: PathBuf,
    root_type: String,
    /// The form as last read (or written).
    doc: Option<Designer>,
    /// Why the file couldn't be read.
    error: Option<String>,
    /// The text the designer last read or wrote (so its own writes aren't read again).
    last_text: String,
    /// Selected components, the primary one first.
    selection: Vec<String>,
    /// The toolbox entry the next click on the form places.
    tool: Option<&'static str>,
    drag: Option<Drag>,
    /// The snap lines of the drag going on.
    guides: Vec<wf::Guide>,
    /// Where the items of strips, menus and grids (and their "Type Here" slots) are, as last drawn.
    item_hits: Vec<(ItemHit, Rect)>,
    /// An item just added from "Type Here": typing its text names it (`&File` → `fileToolStripMenuItem`).
    fresh_item: Option<String>,
    /// A grid field was closed: the designer takes the keyboard back (else shortcuts go to the closed field).
    refocus: bool,
    /// A localized form's `.resx` files (`Form1.resx` and its languages).
    resources: Option<localized::Resources>,
    /// The language shown (`None`: the neutral one).
    language: Option<String>,
    /// The designer's changes, to undo (and undone ones, to redo): code and resources together.
    history: Vec<localized::Step>,
    future: Vec<localized::Step>,
    /// The form while a drag is going on (the model isn't changed until the mouse is let go).
    preview: Option<Designer>,
    /// The page each TabControl shows.
    tabs: HashMap<String, usize>,
    layout: Layout,
    /// Where the form's client area is in the window.
    origin: Rc<Cell<Option<Point<Pixels>>>>,
    focus: FocusHandle,
    input: Entity<TextInput>,
    editing: Option<Row>,
    dropdown: Option<Row>,
    picker_open: bool,
    show_events: bool,
    message: Option<(String, bool)>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<DesignerEvent> for FormDesigner {}

impl Focusable for FormDesigner {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn rgba(c: u32) -> Hsla {
    gpui::rgba(c).into()
}

/// A theme color, else another key's.
fn color(t: &Theme, key: &str, fallback: &str) -> Hsla {
    if t.has(key) { t.c(key) } else { t.c(fallback) }
}

impl FormDesigner {
    pub fn new(editor: Entity<EditorView>, code_path: PathBuf, designer_path: PathBuf, root_type: String, cx: &mut Context<Self>) -> Self {
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
            code_path,
            designer_path,
            root_type,
            doc: None,
            error: None,
            last_text: String::new(),
            selection: Vec::new(),
            tool: None,
            drag: None,
            guides: Vec::new(),
            item_hits: Vec::new(),
            fresh_item: None,
            refocus: false,
            resources: None,
            language: None,
            history: Vec::new(),
            future: Vec::new(),
            preview: None,
            tabs: HashMap::new(),
            layout: Layout::default(),
            origin: Rc::new(Cell::new(None)),
            focus: cx.focus_handle(),
            input,
            editing: None,
            dropdown: None,
            picker_open: false,
            show_events: false,
            message: None,
            _subs: subs,
        };
        this.resources = localized::Resources::load(&this.code_path);
        this.reload(cx);
        this
    }

    pub fn focus(&self, window: &mut Window) {
        window.focus(&self.focus);
    }

    /// The form's class (from the designer file).
    pub fn class_name(&self) -> Option<String> {
        self.doc.as_ref().map(|d| d.class_name.clone())
    }

    #[cfg(test)]
    pub fn model(&self) -> Option<&Designer> {
        self.doc.as_ref()
    }

    #[cfg(test)]
    pub fn selection(&self) -> &[String] {
        &self.selection
    }

    #[cfg(test)]
    pub fn guides(&self) -> &[wf::Guide] {
        &self.guides
    }

    /// Reads the file's text again if it isn't what the designer wrote (typed in the code, undone, reloaded).
    fn reload(&mut self, cx: &mut Context<Self>) {
        let text = self.editor.read(cx).state.buffer.text();
        if text == self.last_text && (self.doc.is_some() || self.error.is_some()) {
            return;
        }
        self.last_text = text;
        match Designer::parse(&self.last_text, &self.root_type) {
            Ok(mut doc) => {
                localized::attach(&mut doc, self.resources.as_ref(), self.language.as_deref());
                self.selection.retain(|n| doc.get(n).is_some());
                if self.selection.is_empty() {
                    self.selection.push(doc.root().name.clone());
                }
                self.doc = Some(doc);
                self.error = None;
            }
            Err(e) => {
                self.error = Some(e);
                self.doc = None;
            }
        }
        self.preview = None;
        self.drag = None;
        cx.notify();
    }

    /// Writes a changed form into the file (as one undo step).
    fn commit(&mut self, mut doc: Designer, cx: &mut Context<Self>) {
        // A localized form's changes to its resources go into the .resx texts (saved with the designer).
        let edits = doc.take_resource_edits();
        let res_before = self.resources.as_ref().map(|r| r.texts.clone());
        if let Some(r) = &mut self.resources
            && let Err(e) = r.apply(&edits)
        {
            self.message = Some((e, true));
        }
        localized::attach(&mut doc, self.resources.as_ref(), self.language.as_deref());
        let res_after = self.resources.as_ref().map(|r| r.texts.clone());
        let text = doc.to_text();
        if text != self.last_text || res_before != res_after {
            self.history.push(localized::Step { code_before: self.last_text.clone(), code_after: text.clone(), res_before, res_after });
            self.future.clear();
        }
        self.selection.retain(|n| doc.get(n).is_some());
        if self.selection.is_empty() {
            self.selection.push(doc.root().name.clone());
        }
        self.doc = Some(doc);
        self.preview = None;
        if text != self.last_text {
            self.last_text = text.clone();
            self.editor.update(cx, |e, cx| e.replace_text_minimal(&text, cx));
        }
        cx.notify();
    }

    /// Ctrl+Z / Ctrl+Y: the designer's last change (to the code, the resources or both) undone or redone;
    /// when the code changed since in other ways (typed in the code), the editor's own history.
    fn undo_redo(&mut self, redo: bool, cx: &mut Context<Self>) {
        let code = self.editor.read(cx).state.buffer.text();
        let res = self.resources.as_ref().map(|r| r.texts.clone());
        let step = if redo { self.future.last() } else { self.history.last() };
        let here = step.is_some_and(|s| if redo { s.code_before == code && s.res_before == res } else { s.code_after == code && s.res_after == res });
        if !here {
            self.history.clear();
            self.future.clear();
            self.editor.update(cx, |e, cx| e.undo_redo(redo, cx));
            return;
        }
        let Some(step) = (if redo { self.future.pop() } else { self.history.pop() }) else { return };
        let (code_to, res_to) = if redo { (step.code_after.clone(), step.res_after.clone()) } else { (step.code_before.clone(), step.res_before.clone()) };
        if let (Some(r), Some(texts)) = (&mut self.resources, res_to) {
            r.texts = texts;
        }
        if code_to != code {
            self.editor.update(cx, |e, cx| e.undo_redo(redo, cx));
            // The editor's history should lead there; if it doesn't (it was cut), the text is set.
            if self.editor.read(cx).state.buffer.text() != code_to {
                self.editor.update(cx, |e, cx| e.replace_text_minimal(&code_to, cx));
            }
        }
        if let Some(doc) = &mut self.doc {
            localized::attach(doc, self.resources.as_ref(), self.language.as_deref());
        }
        if redo { self.history.push(step) } else { self.future.push(step) }
        cx.notify();
    }

    /// Shows a localized form in another language (`None`: the neutral one).
    fn set_language(&mut self, language: Option<String>, cx: &mut Context<Self>) {
        self.finish_edit(true, cx);
        self.language = language;
        if let Some(doc) = &mut self.doc {
            localized::attach(doc, self.resources.as_ref(), self.language.as_deref());
        }
        cx.notify();
    }

    /// Whether the form's .resx files have changes not saved yet.
    pub fn has_unsaved_resources(&self) -> bool {
        self.resources.as_ref().is_some_and(localized::Resources::is_dirty)
    }

    /// Saves the form's changed .resx files (with the designer file).
    pub fn save_resources(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let result = self.resources.as_mut().map_or(Ok(()), localized::Resources::save);
        cx.notify();
        result
    }

    #[cfg(test)]
    pub fn language(&self) -> Option<&str> {
        self.language.as_deref()
    }

    /// Changes the form: `f` gets a copy of the model.
    fn change(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Designer) -> Result<(), String>) {
        let Some(mut doc) = self.doc.clone() else { return };
        match f(&mut doc) {
            Ok(()) => {
                self.message = None;
                self.commit(doc, cx);
            }
            Err(e) => {
                self.message = Some((e, true));
                cx.notify();
            }
        }
    }

    fn primary(&self) -> Option<&str> {
        self.selection.first().map(String::as_str)
    }

    fn select(&mut self, name: &str, cx: &mut Context<Self>) {
        self.finish_edit(true, cx);
        self.selection = vec![name.to_owned()];
        self.dropdown = None;
        self.picker_open = false;
        // A page of a tab control: show it.
        if let Some(doc) = &self.doc
            && let Some((parent, _)) = doc.parent(name)
            && doc.get(parent).is_some_and(|p| p.short_type() == "TabControl")
            && let Some(i) = doc.get(parent).and_then(|p| p.children.iter().position(|c| c.name == name))
        {
            self.tabs.insert(parent.to_owned(), i);
        }
        cx.notify();
    }

    // ───────────── actions ─────────────

    /// Goes to the handler of a component's event, hooking up `<name>_<Event>` first if it has none.
    pub fn go_to_handler(&mut self, name: &str, event: &str, cx: &mut Context<Self>) {
        let Some(doc) = &self.doc else { return };
        let existing = doc.events(name).into_iter().find(|(e, _)| *e == event).map(|(_, h)| h.to_owned());
        let method = match existing {
            Some(h) => h,
            None => {
                let method = format!("{name}_{event}");
                let (owner, ev, m) = (name.to_owned(), event.to_owned(), method.clone());
                self.change(cx, move |d| {
                    d.set_event(&owner, &ev, Some(&m));
                    Ok(())
                });
                method
            }
        };
        cx.emit(DesignerEvent::Handler { method, args: wf::event_info(event).args.to_owned() });
    }

    fn default_event(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(doc) = &self.doc else { return };
        let Some(c) = doc.get(name) else { return };
        let event = if doc.is_root(name) { "Load" } else { wf::default_event(&c.type_name) };
        self.go_to_handler(name, event, cx);
    }

    pub fn delete_selection(&mut self, cx: &mut Context<Self>) {
        let names: Vec<String> = self.selection.clone();
        // An item's strip, menu or grid stays selected.
        let owner = self.doc.as_ref().and_then(|d| names.first().and_then(|n| d.item_owner(n)));
        self.change(cx, |d| {
            for n in &names {
                d.remove(n);
            }
            Ok(())
        });
        if let Some(owner) = owner.filter(|o| self.doc.as_ref().is_some_and(|d| d.get(o).is_some())) {
            self.selection = vec![owner];
            cx.notify();
        }
    }

    /// A click on an item of a strip, menu or grid, or on a "Type Here" slot.
    fn click_item(&mut self, hit: ItemHit, clicks: usize, window: &mut Window, cx: &mut Context<Self>) {
        match hit {
            ItemHit::Item(name) => {
                self.select(&name, cx);
                if clicks >= 2 {
                    let column = self.doc.as_ref().and_then(|d| d.get(&name)).is_some_and(|c| c.short_type().starts_with("DataGridView"));
                    if column {
                        if let Some(p) = wf::property("DataGridViewTextBoxColumn", false, "HeaderText") {
                            self.edit_after_click(Row::Known(p), window, cx);
                        }
                    } else {
                        self.default_event(&name, cx);
                    }
                }
            }
            ItemHit::Add(owner) => self.add_item(&owner, None, window, cx),
        }
    }

    /// Adds an item to a strip, menu item or grid (`type_name`: else what it usually holds), selects it
    /// and starts typing its text (a column's header text) in the grid.
    fn add_item(&mut self, owner: &str, type_name: Option<&'static str>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = &self.doc else { return };
        let Some(ty) = type_name.or_else(|| doc.get(owner).and_then(|c| wf::default_item_type(&c.type_name))) else { return };
        let mut added = None;
        let out = &mut added;
        self.change(cx, |d| {
            *out = d.add_item(owner, ty);
            Ok(())
        });
        let Some(name) = added else { return };
        self.select(&name, cx);
        let column = ty.starts_with("DataGridView");
        if !column {
            self.fresh_item = Some(name);
        }
        if let Some(p) = wf::property(ty, false, if column { "HeaderText" } else { "Text" }) {
            self.edit_after_click(Row::Known(p), window, cx);
        }
    }

    /// Starts typing a property once the click that asked for it is over (the click focuses the form, which
    /// would take the focus from the grid's field).
    fn edit_after_click(&mut self, row: Row, window: &mut Window, cx: &mut Context<Self>) {
        self.show_events = false;
        cx.defer_in(window, move |this, window, cx| this.start_edit(row, window, cx));
    }

    /// Moves the selected item one place back (-1) or on (1) among its owner's items.
    fn move_item(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(name) = self.primary().map(str::to_owned) else { return };
        self.change(cx, |d| {
            d.move_item(&name, delta);
            Ok(())
        });
    }

    /// Ctrl+C: the selected controls (with what is in them) go on the clipboard.
    fn copy_selection(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(doc) = &self.doc else { return false };
        // Items (menu items, columns) are copied with their strip or grid, not on their own.
        let names: Vec<String> = self.selection.iter().filter(|n| !doc.is_item(n)).cloned().collect();
        let fragment = doc.copy(&names);
        if fragment.is_empty() {
            return false;
        }
        let code = fragment.to_code();
        cx.write_to_clipboard(ClipboardItem::new_string(code.clone()));
        cx.set_global(DesignerClipboard { code, fragment });
        true
    }

    fn cut_selection(&mut self, cx: &mut Context<Self>) {
        if self.copy_selection(cx) {
            self.delete_selection(cx);
        }
    }

    /// Ctrl+V: what was copied goes into the selected container (or next to the selected control, or onto
    /// the form), and is selected.
    fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(doc) = &self.doc else { return };
        let Some(clip) = cx.try_global::<DesignerClipboard>() else { return };
        // Something else was copied since (in ByteForge or another app).
        if cx.read_from_clipboard().and_then(|i| i.text()).is_some_and(|t| t.replace("\r\n", "\n") != clip.code) {
            return;
        }
        let fragment = clip.fragment.clone();
        let same_form = fragment.class_name == doc.class_name;
        let root = doc.root().name.clone();
        let primary = self.primary().unwrap_or(&root).to_owned();
        let is_container = |n: &str| doc.get(n).is_some_and(|c| wf::info(&c.type_name).is_some_and(|i| i.container));
        let (owner, slot) = match doc.get(&primary) {
            _ if doc.is_root(&primary) => (root.clone(), String::new()),
            // A copied container still selected: its copy goes next to it.
            _ if same_form && fragment.top.contains(&primary) => doc.parent(&primary).map_or((root.clone(), String::new()), |(p, s)| (p.to_owned(), s.to_owned())),
            Some(c) if c.short_type() == "TabControl" => {
                let shown = self.tabs.get(&primary).copied().unwrap_or(0);
                c.children.get(shown).or(c.children.first()).map_or((root.clone(), String::new()), |p| (p.name.clone(), String::new()))
            }
            Some(c) if c.short_type() == "SplitContainer" => (primary.clone(), "Panel1".into()),
            Some(_) if is_container(&primary) => (primary.clone(), String::new()),
            _ => doc.parent(&primary).map_or((root.clone(), String::new()), |(p, s)| (p.to_owned(), s.to_owned())),
        };
        let mut pasted = Vec::new();
        let out = &mut pasted;
        self.change(cx, |d| {
            *out = d.paste(&fragment, &owner, &slot);
            Ok(())
        });
        if !pasted.is_empty() {
            self.selection = pasted;
            cx.notify();
        }
    }

    /// Ctrl+A: every control on the form (not what is inside containers) and every component.
    fn select_all(&mut self, cx: &mut Context<Self>) {
        let Some(doc) = &self.doc else { return };
        let mut all: Vec<String> = doc.root().children.iter().map(|c| c.name.clone()).collect();
        all.extend(doc.tray().iter().map(|c| c.name.clone()));
        if !all.is_empty() {
            self.finish_edit(true, cx);
            self.selection = all;
            cx.notify();
        }
    }

    /// Lines the selected controls up with the primary one (or gives them its size).
    fn align(&mut self, how: wf::Align, cx: &mut Context<Self>) {
        let Some(doc) = &self.doc else { return };
        let layout = wf::layout(doc, &self.tabs);
        let names: Vec<&String> = self.selection.iter().filter(|n| !doc.is_root(n)).collect();
        let Some(primary) = names.first().and_then(|n| layout.get(n)).map(|p| p.rect) else { return };
        let moves: Vec<(String, Rect, Rect)> = names[1..]
            .iter()
            .filter_map(|n| layout.get(n).filter(|p| !p.arranged))
            .map(|p| (p.name.clone(), p.rect, wf::aligned(primary, p.rect, how)))
            .filter(|(_, from, to)| from != to)
            .collect();
        self.change(cx, |d| {
            for (n, from, to) in &moves {
                if (to.x, to.y) != (from.x, from.y) {
                    let (x, y) = d.location(n);
                    d.set_location(n, (x + to.x - from.x, y + to.y - from.y));
                }
                if (to.w, to.h) != (from.w, from.h) {
                    d.set_size(n, (to.w, to.h));
                }
            }
            Ok(())
        });
    }

    fn reorder(&mut self, front: bool, cx: &mut Context<Self>) {
        let names = self.selection.clone();
        self.change(cx, |d| {
            for n in names.iter().rev() {
                d.reorder(n, front);
            }
            Ok(())
        });
    }

    /// Adds a control from the toolbox: into the selected container (or the form), at `at` in it.
    fn add(&mut self, type_name: &'static str, place: Option<Placement>, cx: &mut Context<Self>) {
        let Some(doc) = &self.doc else { return };
        let (pos, size, owner, slot) = match place {
            Some(p) => (p.at, p.size, p.owner, p.slot),
            None => {
                // Double-clicked in the toolbox: into the selected container, a little off its corner.
                let container = self
                    .primary()
                    .filter(|n| doc.get(n).is_some_and(|c| wf::info(&c.type_name).is_some_and(|i| i.container) && c.short_type() != "TabControl"))
                    .map(str::to_owned)
                    .unwrap_or_else(|| doc.root().name.clone());
                let count = doc.get(&container).map_or(0, |c| c.children.len()) as i32;
                ((12 + 8 * count, 12 + 8 * count), None, container, String::new())
            }
        };
        let mut added = String::new();
        let add = &mut added;
        self.change(cx, |d| {
            let parent = (!d.is_root(&owner)).then_some(owner.as_str());
            let name = d.add_component(type_name, parent, pos, size);
            if !slot.is_empty() && d.parent(&name).is_some_and(|(_, s)| s != slot) {
                d.reparent(&name, &owner, &slot, pos);
            }
            *add = name;
            Ok(())
        });
        if !added.is_empty() {
            self.select(&added, cx);
        }
    }

    // ───────────── the property grid ─────────────

    fn rows(&self, doc: &Designer, name: &str) -> Vec<(&'static str, Vec<Row>)> {
        let Some(c) = doc.get(name) else { return Vec::new() };
        let root = doc.is_root(name);
        let known = wf::properties_for(&c.type_name, root);
        let mut cats: Vec<(&'static str, Vec<Row>)> = Vec::new();
        if !root {
            cats.push(("Design", vec![Row::Name]));
        }
        let mut sorted = known.clone();
        sorted.sort_by_key(|p| (p.category, p.name));
        for p in sorted {
            match cats.iter_mut().find(|(c, _)| *c == p.category) {
                Some((_, rows)) => rows.push(Row::Known(p)),
                None => cats.push((p.category, vec![Row::Known(p)])),
            }
        }
        let others: Vec<Row> = c
            .members
            .iter()
            .filter_map(|m| match m {
                wf::Member::Prop { name, .. } if name != "Name" && !known.iter().any(|p| p.name == name) => Some(Row::Other(name.clone())),
                _ => None,
            })
            .collect();
        if !others.is_empty() {
            cats.push(("Other (as code)", others));
        }
        cats.sort_by_key(|(c, _)| (*c != "Design", *c == "Other (as code)", *c));
        cats
    }

    fn row_value(doc: &Designer, owner: &str, row: &Row) -> wf::GridValue {
        match row {
            Row::Name => wf::GridValue { text: owner.to_owned(), set: true },
            Row::Known(p) => doc.grid_value(owner, p),
            Row::Other(n) => wf::GridValue { text: doc.prop(owner, n).unwrap_or("").to_owned(), set: true },
        }
    }

    /// A click on a row's value: a switch flips, a choice opens its list, the rest is typed.
    fn click_value(&mut self, row: Row, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(doc), Some(owner)) = (self.doc.clone(), self.primary().map(str::to_owned)) else { return };
        if self.editing.as_ref() == Some(&row) {
            return;
        }
        self.finish_edit(true, cx);
        if let Row::Known(p) = &row {
            match p.kind {
                PropKind::Bool => {
                    let now = doc.grid_value(&owner, p).text == "true";
                    let p = *p;
                    self.change(cx, |d| d.set_from_grid(&owner, p, if now { "false" } else { "true" }));
                    return;
                }
                PropKind::Enum(..) | PropKind::Reference | PropKind::Color | PropKind::Anchor => {
                    self.dropdown = if self.dropdown.as_ref() == Some(&row) { None } else { Some(row) };
                    cx.notify();
                    return;
                }
                _ => {}
            }
        }
        self.start_edit(row, window, cx);
    }

    fn start_edit(&mut self, row: Row, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(doc), Some(owner)) = (&self.doc, self.primary()) else { return };
        let text = match &row {
            Row::Known(p) => doc.grid_edit_text(owner, p),
            _ => Self::row_value(doc, owner, &row).text,
        };
        let multiline = matches!(&row, Row::Known(p) if p.kind == PropKind::Items);
        self.input = cx.new(|cx| {
            let mut i = if multiline { TextInput::multiline("one item per line (Ctrl+Enter to apply)", 8, cx) } else { TextInput::new("", cx) };
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
        let Some(owner) = self.primary().map(str::to_owned) else { return };
        let value = self.input.read(cx).text().to_owned();
        match row {
            Row::Name => {
                let new = value.trim().to_owned();
                self.rename_selected(&owner, &new, cx);
            }
            Row::Known(p) => {
                self.change(cx, |d| d.set_from_grid(&owner, p, &value));
                // A menu item added from "Type Here" gets its name from what is typed, as in Visual Studio.
                if p.name == "Text"
                    && self.message.is_none()
                    && self.fresh_item.take().as_deref() == Some(owner.as_str())
                    && let Some(doc) = &self.doc
                    && let Some(ty) = doc.get(&owner).map(|c| c.type_name.clone())
                    && let Some(new) = doc.item_name_for_text(&value, &ty).filter(|n| doc.get(n).is_none())
                {
                    self.rename_selected(&owner, &new, cx);
                }
            }
            Row::Other(name) => {
                let v = value.trim().to_owned();
                self.change(cx, |d| {
                    d.set_prop(&owner, &name, (!v.is_empty()).then_some(v.as_str()));
                    Ok(())
                });
            }
        }
    }

    /// Renames the primary selected component; it stays selected under its new name.
    fn rename_selected(&mut self, old: &str, new: &str, cx: &mut Context<Self>) {
        let rest: Vec<String> = self.selection.iter().filter(|n| *n != old).cloned().collect();
        self.change(cx, |d| d.rename(old, new));
        if self.message.is_none() {
            let root = self.doc.as_ref().map(|d| d.root().name.clone()).unwrap_or_default();
            self.selection = std::iter::once(new.to_owned()).chain(rest.into_iter().filter(|n| *n != root)).collect();
            cx.notify();
        }
    }

    fn pick(&mut self, row: &'static PropInfo, value: String, cx: &mut Context<Self>) {
        let Some(owner) = self.primary().map(str::to_owned) else { return };
        self.dropdown = None;
        self.change(cx, |d| d.set_from_grid(&owner, row, &value));
    }

    // ───────────── the mouse on the form ─────────────

    fn to_form(&self, p: Point<Pixels>) -> Option<(i32, i32)> {
        let o = self.origin.get()?;
        Some((f32::from(p.x - o.x).round() as i32, f32::from(p.y - o.y).round() as i32))
    }

    fn handle_rect(rect: Rect, h: Handle, size: i32) -> Rect {
        let cx = rect.x + (h.0 as i32 + 1) * rect.w / 2;
        let cy = rect.y + (h.1 as i32 + 1) * rect.h / 2;
        Rect::new(cx - size / 2, cy - size / 2, size, size)
    }

    /// The selection's resize handles: (component, handle, its square).
    fn handles(&self, doc: &Designer, size: i32) -> Vec<(String, Handle, Rect, Rect)> {
        let Some(name) = self.primary() else { return Vec::new() };
        if self.selection.len() > 1 {
            return Vec::new();
        }
        if doc.is_root(name) {
            let (w, h) = doc.size(name);
            let rect = Rect::new(0, 0, w, h);
            return ROOT_HANDLES.iter().map(|&hd| (name.to_owned(), hd, Self::handle_rect(rect, hd, size), rect)).collect();
        }
        match self.layout.get(name) {
            Some(p) if !wf::is_component(&doc.get(name).map(|c| c.type_name.clone()).unwrap_or_default()) => {
                ALL_HANDLES.iter().map(|&hd| (name.to_owned(), hd, Self::handle_rect(p.rect, hd, size), p.rect)).collect()
            }
            _ => Vec::new(),
        }
    }

    /// The tab headers of a TabControl: (page, header rectangle).
    fn tab_headers(doc: &Designer, name: &str, rect: Rect) -> Vec<(String, Rect)> {
        let Some(c) = doc.get(name) else { return Vec::new() };
        let mut x = rect.x + 2;
        c.children
            .iter()
            .map(|page| {
                let text = doc.text_of(&page.name).unwrap_or_else(|| page.name.clone());
                let w = text.chars().count() as i32 * 7 + 14;
                let r = Rect::new(x, rect.y + 2, w, 20);
                x += w;
                (page.name.clone(), r)
            })
            .collect()
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        self.finish_edit(true, cx);
        self.dropdown = None;
        self.picker_open = false;
        let (Some(doc), Some(p)) = (self.doc.clone(), self.to_form(event.position)) else { return };
        if let Some(tool) = self.tool {
            self.drag = Some(Drag { kind: DragKind::Create { type_name: tool }, start: p, now: p, moved: false, snap: false });
            return;
        }
        // Items of strips, menus and grids (drawn over the controls).
        if let Some(hit) = self.item_hits.iter().rev().find(|(_, r)| r.contains(p.0, p.1)).map(|(h, _)| h.clone()) {
            self.click_item(hit, event.click_count, window, cx);
            return;
        }
        let hs = Theme::global(cx).n("formDesigner.handleSize", 7.0) as i32 + 2;
        if let Some((name, handle, _, rect)) = self.handles(&doc, hs).into_iter().find(|(_, _, r, _)| r.inset(-1, -1, -1, -1).contains(p.0, p.1)) {
            self.drag = Some(Drag { kind: DragKind::Resize { name, handle, rect }, start: p, now: p, moved: false, snap: !event.modifiers.alt });
            return;
        }
        let hit = self.layout.hit(p.0, p.1).cloned();
        let Some(hit) = hit else {
            // The form itself (its client area or title bar).
            let root = doc.root().name.clone();
            self.select(&root, cx);
            if event.click_count >= 2 {
                self.default_event(&root, cx);
            }
            return;
        };
        // A tab header shows its page.
        if doc.get(&hit.name).is_some_and(|c| c.short_type() == "TabControl")
            && let Some((page, _)) = Self::tab_headers(&doc, &hit.name, hit.rect).into_iter().find(|(_, r)| r.contains(p.0, p.1))
        {
            self.select(&page, cx);
            return;
        }
        let name = hit.name.clone();
        if event.modifiers.control || event.modifiers.shift || event.modifiers.platform {
            if let Some(i) = self.selection.iter().position(|n| *n == name) {
                if self.selection.len() > 1 {
                    self.selection.remove(i);
                }
            } else {
                self.selection.retain(|n| !doc.is_root(n));
                self.selection.push(name);
            }
            cx.notify();
            return;
        }
        if !self.selection.contains(&name) {
            self.select(&name, cx);
        } else if self.selection[0] != name {
            // Clicked one of several: it becomes the primary one.
            self.selection.retain(|n| *n != name);
            self.selection.insert(0, name.clone());
        }
        if event.click_count >= 2 {
            self.default_event(&name, cx);
            return;
        }
        let names: Vec<String> = self.selection.iter().filter(|n| !doc.is_root(n) && !doc.is_item(n)).cloned().collect();
        if !names.is_empty() {
            self.drag = Some(Drag { kind: DragKind::Move { names }, start: p, now: p, moved: false, snap: !event.modifiers.alt });
        }
        cx.notify();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.to_form(event.position) else { return };
        let Some(drag) = &mut self.drag else { return };
        if event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        drag.now = p;
        if !matches!(drag.kind, DragKind::Create { .. }) {
            drag.snap = !event.modifiers.alt;
        }
        if !drag.moved && ((p.0 - drag.start.0).abs() > DRAG_THRESHOLD || (p.1 - drag.start.1).abs() > DRAG_THRESHOLD) {
            drag.moved = true;
        }
        if drag.moved {
            let (preview, guides) = self.drag.as_ref().and_then(|d| self.dragged_model(d)).map_or((None, Vec::new()), |(d, g)| (Some(d), g));
            self.preview = preview;
            self.guides = guides;
            cx.notify();
        }
    }

    /// The controls in `area` something snaps to, leaving out `skip`.
    fn snap_targets(layout: &Layout, root: &str, area: &wf::Area, skip: &[String]) -> Vec<Rect> {
        layout
            .controls
            .iter()
            .filter(|c| c.parent.as_deref().unwrap_or(root) == area.owner && c.slot == area.slot && !skip.contains(&c.name) && c.visible.w > 0 && c.visible.h > 0)
            .map(|c| c.rect)
            .collect()
    }

    /// The form as it would be if the mouse were let go now, and the snap lines it lines up with.
    fn dragged_model(&self, drag: &Drag) -> Option<(Designer, Vec<wf::Guide>)> {
        let doc = self.doc.as_ref()?;
        let mut d = doc.clone();
        let (mut dx, mut dy) = (drag.now.0 - drag.start.0, drag.now.1 - drag.start.1);
        let mut guides = Vec::new();
        let base = if drag.snap { Some(wf::layout(doc, &self.tabs)) } else { None };
        let root = doc.root().name.clone();
        match &drag.kind {
            DragKind::Move { names } => {
                // The first one snaps (in the container it would be dropped in); the others move with it.
                if let Some(base) = &base
                    && let Some(p) = names.first().and_then(|n| base.get(n)).filter(|p| !p.arranged)
                    && let Some(area) = base.area_at(doc, drag.now.0, drag.now.1, names).or_else(|| base.area(p.parent.as_deref().unwrap_or(&root), &p.slot))
                {
                    let moved = Rect::new(p.rect.x + dx, p.rect.y + dy, p.rect.w, p.rect.h);
                    let others = Self::snap_targets(base, &root, area, names);
                    let (sx, sy, g) = wf::snap(moved, wf::Edges::ALL, &others, area.bounds, wf::snap_padding(doc, &area.owner), SNAP_THRESHOLD);
                    (dx, dy, guides) = (dx + sx, dy + sy, g);
                }
                for n in names {
                    // Only what isn't inside another moved control moves itself.
                    if names.iter().any(|o| o != n && d.is_within(n, o)) {
                        continue;
                    }
                    let (x, y) = d.location(n);
                    d.set_location(n, (x + dx, y + dy));
                }
            }
            DragKind::Resize { name, handle, rect } => {
                let mut r = resized(*rect, *handle, dx, dy);
                if let Some(base) = &base
                    && let Some(p) = base.get(name).filter(|p| !p.arranged)
                    && let Some(area) = base.area(p.parent.as_deref().unwrap_or(&root), &p.slot)
                {
                    let edges = wf::Edges { left: handle.0 == -1, top: handle.1 == -1, right: handle.0 == 1, bottom: handle.1 == 1 };
                    let others = Self::snap_targets(base, &root, area, std::slice::from_ref(name));
                    let (sx, sy, g) = wf::snap(r, edges, &others, area.bounds, wf::snap_padding(doc, &area.owner), SNAP_THRESHOLD);
                    r = resized(r, *handle, sx, sy);
                    guides = g;
                }
                if !d.is_root(name) {
                    let (x, y) = d.location(name);
                    d.set_location(name, (x + r.x - rect.x, y + r.y - rect.y));
                }
                d.set_size(name, (r.w, r.h));
            }
            DragKind::Create { .. } => return None,
        }
        Some((d, guides))
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(mut drag) = self.drag.take() else { return };
        self.guides.clear();
        if let Some(p) = self.to_form(event.position) {
            drag.now = p;
        }
        let Some(doc) = self.doc.clone() else { return };
        match &drag.kind {
            DragKind::Create { type_name } => {
                let type_name = *type_name;
                let (x0, y0) = drag.start;
                let (x1, y1) = drag.now;
                let size = drag.moved.then(|| ((x1 - x0).abs().max(8), (y1 - y0).abs().max(8)));
                let at = (x0.min(x1), y0.min(y1));
                let area = self.layout.area_at(&doc, at.0, at.1, &[]).cloned();
                let (owner, slot, origin) = area.map_or((doc.root().name.clone(), String::new(), (0, 0)), |a| (a.owner, a.slot, a.origin));
                if !event.modifiers.shift {
                    self.tool = None;
                }
                self.add(type_name, Some(Placement { at: (at.0 - origin.0, at.1 - origin.1), size, owner, slot }), cx);
            }
            DragKind::Move { names } if drag.moved => {
                let names = names.clone();
                let Some((mut d, _)) = self.dragged_model(&drag) else { return };
                // Dropped on another container: it moves there.
                let layout = wf::layout(&d, &self.tabs);
                let target = self.layout.area_at(&doc, drag.now.0, drag.now.1, &names).cloned();
                for n in &names {
                    if names.iter().any(|o| o != n && d.is_within(n, o)) {
                        continue;
                    }
                    let Some(placed) = layout.get(n) else { continue };
                    if let Some(area) = &target {
                        let current = d.parent(n).map(|(p, s)| (p.to_owned(), s.to_owned()));
                        if current.as_ref().is_none_or(|(p, s)| *p != area.owner || *s != area.slot) {
                            let at = (placed.rect.x - area.origin.0, placed.rect.y - area.origin.1);
                            d.reparent(n, &area.owner, &area.slot, at);
                        }
                    }
                }
                self.commit(d, cx);
            }
            DragKind::Resize { .. } if drag.moved => {
                if let Some((d, _)) = self.dragged_model(&drag) {
                    self.commit(d, cx);
                }
            }
            _ => {}
        }
        self.preview = None;
        cx.notify();
    }

    fn on_drop_tool(&mut self, tool: &DraggedTool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.clone() else { return };
        let Some(p) = self.to_form(window.mouse_position()) else { return };
        let area = self.layout.area_at(&doc, p.0, p.1, &[]).cloned();
        let (owner, slot, origin) = area.map_or((doc.root().name.clone(), String::new(), (0, 0)), |a| (a.owner, a.slot, a.origin));
        self.tool = None;
        self.add(tool.0, Some(Placement { at: (p.0 - origin.0, p.1 - origin.1), size: None, owner, slot }), cx);
        window.focus(&self.focus);
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.input.focus_handle(cx).is_focused(window) || self.doc.is_none() {
            return;
        }
        let key = &event.keystroke;
        let m = key.modifiers;
        let secondary = m.secondary() && !m.alt;
        let step = if secondary { 8 } else { 1 };
        let arrow = match key.key.as_str() {
            "left" => Some((-step, 0)),
            "right" => Some((step, 0)),
            "up" => Some((0, -step)),
            "down" => Some((0, step)),
            _ => None,
        };
        if let Some((dx, dy)) = arrow {
            let names: Vec<String> = self.selection.clone();
            let resize = m.shift;
            self.change(cx, |d| {
                for n in &names {
                    // Items have no location or size of their own.
                    if d.is_item(n) {
                        continue;
                    }
                    if resize {
                        let (w, h) = d.size(n);
                        d.set_size(n, (w + dx, h + dy));
                    } else if !d.is_root(n) {
                        let (x, y) = d.location(n);
                        d.set_location(n, (x + dx, y + dy));
                    }
                }
                Ok(())
            });
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
                    && let Some(doc) = &self.doc
                    && let Some(name) = self.primary()
                {
                    let up = doc.item_owner(name).or_else(|| doc.parent(name).map(|(p, _)| p.to_owned())).unwrap_or_else(|| doc.root().name.clone());
                    self.select(&up, cx);
                }
                cx.notify();
                cx.stop_propagation();
            }
            "z" if secondary => {
                self.undo_redo(m.shift, cx);
                cx.stop_propagation();
            }
            "y" if secondary => {
                self.undo_redo(true, cx);
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
            "a" if secondary && !m.shift => {
                self.select_all(cx);
                cx.stop_propagation();
            }
            "enter" if !m.modified() => {
                if let Some(name) = self.primary().map(str::to_owned) {
                    self.default_event(&name, cx);
                }
                cx.stop_propagation();
            }
            _ => {}
        }
    }
}

/// The shadow of an open menu on the form: Windows', not the theme's (the form shows how it will run).
fn menu_shadow() -> Vec<gpui::BoxShadow> {
    vec![gpui::BoxShadow { color: gpui::hsla(0., 0., 0., 0.22), offset: gpui::point(px(0.), px(2.)), blur_radius: px(8.), spread_radius: px(0.) }]
}

/// `rect` with the side(s) of `handle` dragged by (dx, dy).
fn resized(rect: Rect, handle: Handle, dx: i32, dy: i32) -> Rect {
    let mut r = rect;
    match handle.0 {
        -1 => {
            let dx = dx.min(rect.w - 1);
            r.x += dx;
            r.w -= dx;
        }
        1 => r.w = (rect.w + dx).max(1),
        _ => {}
    }
    match handle.1 {
        -1 => {
            let dy = dy.min(rect.h - 1);
            r.y += dy;
            r.h -= dy;
        }
        1 => r.h = (rect.h + dy).max(1),
        _ => {}
    }
    r
}

/// Today as the date picker shows it (`Tuesday, November 24, 2026` / `11/24/2026`).
fn today(long: bool) -> String {
    let days = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86400) as i64;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    if !long {
        return format!("{month}/{day}/{year}");
    }
    const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    const DAYS: [&str; 7] = ["Thursday", "Friday", "Saturday", "Sunday", "Monday", "Tuesday", "Wednesday"];
    format!("{}, {} {day}, {year}", DAYS[days.rem_euclid(7) as usize], MONTHS[(month - 1) as usize])
}

// ───────────── drawing ─────────────

/// How a control looks: Windows 11's default colors, not the theme's (it is the form as it will run).
struct Look<'a> {
    doc: &'a Designer,
    font_family: SharedString,
}

impl Look<'_> {
    fn color(&self, name: &str, prop: &str, fallback: u32) -> Hsla {
        rgba(self.doc.prop(name, prop).and_then(wf::parse_color).unwrap_or(fallback))
    }

    /// The control's font size in pixels, bold and italic (its own Font, else the form's, else 9pt).
    fn font(&self, name: &str) -> (f32, bool, bool) {
        let root = &self.doc.root().name;
        let f = self.doc.prop(name, "Font").or_else(|| self.doc.prop(root, "Font")).and_then(wf::parse_font);
        f.map_or((12.0, false, false), |f| (f.size * 96.0 / 72.0, f.bold, f.italic))
    }

    fn text_el(&self, name: &str, text: String, fg: Hsla) -> gpui::Div {
        let (size, bold, italic) = self.font(name);
        div()
            .text_size(px(size))
            .line_height(px(size * 1.3))
            .font_family(self.font_family.clone())
            .text_color(fg)
            .when(bold, |d| d.font_weight(gpui::FontWeight::BOLD))
            .when(italic, |d| d.italic())
            .whitespace_nowrap()
            .child(text)
    }

    /// Aligns a box's content as a ContentAlignment says.
    fn align(d: gpui::Div, alignment: &str) -> gpui::Div {
        let d = d.flex();
        let d = if alignment.starts_with("Top") {
            d.items_start()
        } else if alignment.starts_with("Bottom") {
            d.items_end()
        } else {
            d.items_center()
        };
        if alignment.ends_with("Left") {
            d.justify_start()
        } else if alignment.ends_with("Right") {
            d.justify_end()
        } else {
            d.justify_center()
        }
    }

    fn element(&self, p: &Placed, back: Hsla, tabs: &HashMap<String, usize>) -> AnyElement {
        let doc = self.doc;
        let name = p.name.as_str();
        let Some(c) = doc.get(name) else { return div().into_any_element() };
        let ty = c.short_type();
        let text = doc.text_of(name).unwrap_or_default();
        let fg = self.color(name, "ForeColor", 0x000000ff);
        let enabled = doc.prop(name, "Enabled").and_then(wf::parse_bool) != Some(false);
        let fg = if enabled { fg } else { rgba(0x6d6d6dff) };
        let bg_or = |fallback: u32| self.color(name, "BackColor", fallback);
        let border = rgba(0x7a7a7aff);
        let light_border = rgba(0xadadadff);
        let field = |content: gpui::Div| div().size_full().bg(bg_or(0xffffffff)).border_1().border_color(border).overflow_hidden().child(content);
        let align = |d: gpui::Div, default: &str| Self::align(d, &doc.prop(name, "TextAlign").and_then(wf::parse_enum).unwrap_or_else(|| default.to_owned()));
        let box_mark = |checked: bool, round: bool| {
            div()
                .flex_none()
                .size(px(13.))
                .mr(px(4.))
                .bg(rgba(if checked { 0x005fb8ff } else { 0xffffffff }))
                .border_1()
                .border_color(rgba(if checked { 0x005fb8ff } else { 0x333333ff }))
                .when(round, |d| d.rounded_full())
                .when(!round, |d| d.rounded(px(2.)))
                .flex()
                .items_center()
                .justify_center()
                .when(checked, |d| {
                    if round {
                        d.child(div().size(px(5.)).rounded_full().bg(rgba(0xffffffff)))
                    } else {
                        d.child(div().text_size(px(10.)).text_color(rgba(0xffffffff)).child("✓"))
                    }
                })
        };
        let lines = |items: Vec<String>, checks: bool| {
            div().flex().flex_col().p(px(1.)).children(items.into_iter().map(move |i| {
                div().flex().items_center().h(px(17.)).px(px(2.)).when(checks, |d| d.child(box_mark(false, false))).child(self.text_el(name, i, fg))
            }))
        };
        let designer_outline = |d: gpui::Div| d.border_1().border_dashed().border_color(rgba(0x80808099));
        match ty {
            "Button" => align(div().size_full().bg(bg_or(0xfdfdfdff)).border_1().border_color(light_border).rounded(px(4.)).px(px(4.)), "MiddleCenter")
                .child(self.text_el(name, text, fg))
                .into_any_element(),
            "Label" | "LinkLabel" => {
                let fg = if ty == "LinkLabel" { rgba(0x0066ccff) } else { fg };
                let el = self.text_el(name, text, fg).when(ty == "LinkLabel", |d| d.underline());
                let bordered = doc.prop(name, "BorderStyle").and_then(wf::parse_enum).is_some_and(|b| b != "None");
                align(div().size_full().when(doc.prop(name, "BackColor").is_some(), |d| d.bg(bg_or(0))).when(bordered, |d| d.border_1().border_color(border)), "TopLeft")
                    .child(el)
                    .into_any_element()
            }
            "TextBox" | "RichTextBox" | "MaskedTextBox" => {
                let password = doc.prop(name, "PasswordChar").is_some() || doc.prop(name, "UseSystemPasswordChar").and_then(wf::parse_bool) == Some(true);
                let shown = if password { "●".repeat(text.chars().count()) } else { text };
                let placeholder = doc.prop(name, "PlaceholderText").and_then(wf::parse_string).filter(|_| shown.is_empty());
                let multiline = ty == "RichTextBox" || doc.prop(name, "Multiline").and_then(wf::parse_bool) == Some(true);
                let content = match placeholder {
                    Some(ph) => self.text_el(name, ph, rgba(0x6d6d6dff)),
                    None => self.text_el(name, shown, fg),
                };
                field(div().size_full().flex().px(px(3.)).when(!multiline, |d| d.items_center()).when(multiline, |d| d.pt(px(2.))).child(content)).into_any_element()
            }
            "ComboBox" | "DateTimePicker" | "NumericUpDown" | "DomainUpDown" => {
                let value = match ty {
                    "DateTimePicker" => today(doc.prop(name, "Format").and_then(wf::parse_enum).is_none_or(|f| f == "Long")),
                    "NumericUpDown" => doc.prop(name, "Value").map(wf::describe_decimal).unwrap_or_else(|| "0".into()),
                    _ => text,
                };
                let arrow = if ty == "NumericUpDown" { "⇕" } else { "⌄" };
                field(
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .child(div().flex_1().min_w_0().px(px(3.)).child(self.text_el(name, value, fg)))
                        .child(div().flex_none().w(px(17.)).h_full().flex().items_center().justify_center().text_size(px(11.)).text_color(rgba(0x333333ff)).child(arrow)),
                )
                .into_any_element()
            }
            "ListBox" | "CheckedListBox" => field(lines(doc.list_items(name), ty == "CheckedListBox")).into_any_element(),
            "CheckBox" | "RadioButton" => {
                let checked = doc.prop(name, "Checked").and_then(wf::parse_bool) == Some(true);
                div().size_full().flex().items_center().when(doc.prop(name, "BackColor").is_some(), |d| d.bg(bg_or(0))).child(box_mark(checked, ty == "RadioButton")).child(self.text_el(name, text, fg)).into_any_element()
            }
            "ProgressBar" => {
                let value = doc.prop(name, "Value").and_then(wf::parse_int).unwrap_or(0) as f32;
                let max = doc.prop(name, "Maximum").and_then(wf::parse_int).unwrap_or(100).max(1) as f32;
                let min = doc.prop(name, "Minimum").and_then(wf::parse_int).unwrap_or(0) as f32;
                let frac = ((value - min) / (max - min).max(1.0)).clamp(0.0, 1.0);
                div().size_full().bg(rgba(0xe6e6e6ff)).border_1().border_color(rgba(0xbcbcbcff)).child(div().h_full().w(gpui::relative(frac)).bg(rgba(0x06b025ff))).into_any_element()
            }
            "TrackBar" => div()
                .size_full()
                .relative()
                .child(div().absolute().left(px(8.)).right(px(8.)).top(px(12.)).h(px(4.)).rounded(px(2.)).bg(rgba(0xc4c4c4ff)))
                .child(div().absolute().left(px(6.)).top(px(6.)).w(px(11.)).h(px(16.)).rounded(px(3.)).bg(rgba(0x005fb8ff)))
                .into_any_element(),
            "GroupBox" => {
                let caption = self.text_el(name, text.clone(), fg).px(px(2.)).bg(back);
                div()
                    .size_full()
                    .relative()
                    .when(doc.prop(name, "BackColor").is_some(), |d| d.bg(bg_or(0)))
                    .child(div().absolute().left_0().right_0().top(px(7.)).bottom_0().border_1().border_color(rgba(0xdcdcdcff)).rounded(px(3.)))
                    .when(!text.is_empty(), |d| d.child(div().absolute().left(px(6.)).top_0().child(caption)))
                    .into_any_element()
            }
            "TabControl" => {
                let shown = tabs.get(name).copied().or_else(|| doc.prop(name, "SelectedIndex").and_then(wf::parse_int).map(|n| n.max(0) as usize)).unwrap_or(0);
                let headers = FormDesigner::tab_headers(doc, name, p.rect);
                div()
                    .size_full()
                    .relative()
                    .child(div().absolute().left_0().right_0().top(px(21.)).bottom_0().bg(rgba(0xffffffff)).border_1().border_color(rgba(0xd9d9d9ff)))
                    .children(headers.into_iter().enumerate().map(|(i, (page, r))| {
                        let label = doc.text_of(&page).unwrap_or(page);
                        let selected = i == shown;
                        div()
                            .absolute()
                            .left(px((r.x - p.rect.x) as f32))
                            .top(px(if selected { 0. } else { 2. }))
                            .w(px(r.w as f32))
                            .h(px(if selected { 22. } else { 20. }))
                            .bg(rgba(if selected { 0xffffffff } else { 0xf0f0f0ff }))
                            .border_1()
                            .border_b_0()
                            .border_color(rgba(0xd9d9d9ff))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(self.text_el(name, label, fg))
                    }))
                    .into_any_element()
            }
            "TabPage" => div().size_full().bg(bg_or(0xffffffff)).into_any_element(),
            "DataGridView" => {
                let line = rgba(0xe0e0e0ff);
                let headers = strips::column_headers(doc, name, p.rect);
                div()
                    .size_full()
                    .relative()
                    .bg(rgba(0xabababff))
                    .border_1()
                    .border_color(border)
                    .child(div().absolute().left_0().top_0().right_0().h(px(strips::HEADER_HEIGHT as f32)).bg(rgba(0xffffffff)).border_b_1().border_color(line))
                    .child(div().absolute().left_0().top_0().w(px(strips::ROW_HEADERS as f32)).h(px(strips::HEADER_HEIGHT as f32)).border_r_1().border_color(line))
                    .children(headers.into_iter().map(|(col, r)| {
                        div()
                            .absolute()
                            .left(px((r.x - p.rect.x - 1) as f32))
                            .top_0()
                            .w(px(r.w as f32))
                            .h(px(r.h as f32))
                            .px(px(4.))
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .border_r_1()
                            .border_color(line)
                            .child(self.text_el(name, strips::label(doc, &col), fg))
                    }))
                    .into_any_element()
            }
            "MenuStrip" | "StatusStrip" | "ToolStrip" => {
                let items = strips::strip_items(doc, name, p.rect);
                let empty = items.is_empty();
                div()
                    .size_full()
                    .relative()
                    .bg(bg_or(if ty == "MenuStrip" { 0xf5f5f5ff } else { 0xf0f0f0ff }))
                    .when(ty == "StatusStrip", |d| d.border_t_1().border_color(rgba(0xdcdcdcff)))
                    .children(items.into_iter().map(|(item, r)| {
                        let at = div().absolute().left(px((r.x - p.rect.x) as f32)).top(px((r.y - p.rect.y) as f32)).w(px(r.w as f32)).h(px(r.h as f32));
                        if strips::is_separator(doc, &item) {
                            at.flex().justify_center().py(px(3.)).child(div().w(px(1.)).h_full().bg(rgba(0xc8c8c8ff)))
                        } else {
                            let enabled = doc.prop(&item, "Enabled").and_then(wf::parse_bool) != Some(false);
                            at.flex().items_center().justify_center().child(self.text_el(name, strips::label(doc, &item), if enabled { fg } else { rgba(0x8a8a8aff) }))
                        }
                    }))
                    .when(empty, |d| d.child(div().absolute().left(px(6.)).top_0().bottom_0().flex().items_center().child(self.text_el(name, format!("({ty})"), rgba(0x8a8a8aff)))))
                    .into_any_element()
            }
            "PictureBox" => {
                let bordered = doc.prop(name, "BorderStyle").and_then(wf::parse_enum).is_some_and(|b| b != "None");
                div()
                    .size_full()
                    .when(doc.prop(name, "BackColor").is_some(), |d| d.bg(bg_or(0)))
                    .map(|d| if bordered { d.border_1().border_color(border) } else { designer_outline(d) })
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(doc.prop(name, "Image").is_none(), |d| d.child(div().text_size(px(11.)).text_color(rgba(0x8a8a8aff)).child("🖼")))
                    .into_any_element()
            }
            "Panel" | "FlowLayoutPanel" | "TableLayoutPanel" | "SplitContainer" | "UserControl" => {
                let b = doc.prop(name, "BorderStyle").and_then(wf::parse_enum);
                let bordered = b.as_deref().is_some_and(|b| b != "None");
                let mut d = div().size_full().relative().when(doc.prop(name, "BackColor").is_some(), |d| d.bg(bg_or(0)));
                d = if bordered { d.border_1().border_color(border) } else { designer_outline(d) };
                if ty == "TableLayoutPanel" {
                    let cols = doc.prop(name, "ColumnCount").and_then(wf::parse_int).unwrap_or(1).max(1);
                    let rows = doc.prop(name, "RowCount").and_then(wf::parse_int).unwrap_or(1).max(1);
                    let line = rgba(0x80808066);
                    d = d
                        .children((1..cols).map(|i| div().absolute().top_0().bottom_0().left(gpui::relative(i as f32 / cols as f32)).w(px(1.)).bg(line)))
                        .children((1..rows).map(|i| div().absolute().left_0().right_0().top(gpui::relative(i as f32 / rows as f32)).h(px(1.)).bg(line)));
                }
                if ty == "SplitContainer" {
                    let horizontal = doc.prop(name, "Orientation").and_then(wf::parse_enum).as_deref() == Some("Horizontal");
                    let size = if horizontal { p.rect.h } else { p.rect.w };
                    let dist = doc.prop(name, "SplitterDistance").and_then(wf::parse_int).map_or(size / 2, |n| n as i32) as f32;
                    let bar = div().absolute().bg(rgba(0xe5e5e5ff));
                    d = d.child(if horizontal { bar.left_0().right_0().top(px(dist)).h(px(4.)) } else { bar.top_0().bottom_0().left(px(dist)).w(px(4.)) });
                }
                d.into_any_element()
            }
            _ => div()
                .size_full()
                .bg(rgba(0xffffff80))
                .border_1()
                .border_color(border)
                .flex()
                .items_center()
                .justify_center()
                .overflow_hidden()
                .child(self.text_el(name, if text.is_empty() { ty.to_owned() } else { text }, fg))
                .into_any_element(),
        }
    }
}

impl FormDesigner {
    /// The open menus, the "Type Here" slots and the frames of selected items, over the form.
    fn render_item_overlays(&self, doc: &Designer, menus: &[strips::Menu], selection: Hsla, t: &Theme) -> Vec<AnyElement> {
        let mut out: Vec<AnyElement> = Vec::new();
        let font = t.ui_font.clone();
        let text = |s: String, color: u32| div().text_size(px(12.)).font_family(font.clone()).text_color(rgba(color)).whitespace_nowrap().child(s);
        let place = |r: Rect| div().absolute().left(px(r.x as f32)).top(px(r.y as f32)).w(px(r.w as f32)).h(px(r.h as f32));
        let type_here = |r: Rect| place(r).px(px(6.)).flex().items_center().bg(rgba(0xffffffff)).border_1().border_dashed().border_color(rgba(0xa0a0a0ff)).child(text("Type Here".into(), 0x8a8a8aff).italic());
        // The add slots of strips and grids (a menu has its own as its last row).
        for (hit, r) in &self.item_hits {
            if let ItemHit::Add(owner) = hit
                && !menus.iter().any(|m| m.owner == *owner)
            {
                let grid = doc.get(owner).is_some_and(|c| c.short_type() == "DataGridView");
                out.push(if grid {
                    place(*r).flex().items_center().justify_center().bg(rgba(0xffffffff)).border_1().border_dashed().border_color(rgba(0xa0a0a0ff)).child(text("+".into(), 0x5a5a5aff)).into_any_element()
                } else {
                    type_here(*r).into_any_element()
                });
            }
        }
        for m in menus {
            let rows: Vec<AnyElement> = m
                .rows
                .iter()
                .map(|(hit, r)| {
                    let local = Rect::new(r.x - m.rect.x, r.y - m.rect.y, r.w, r.h);
                    match hit {
                        ItemHit::Add(_) => type_here(local).into_any_element(),
                        ItemHit::Item(n) if strips::is_separator(doc, n) => place(local).flex().items_center().pl(px(28.)).pr(px(4.)).child(div().flex_1().h(px(1.)).bg(rgba(0xd8d8d8ff))).into_any_element(),
                        ItemHit::Item(n) => {
                            let checked = doc.prop(n, "Checked").and_then(wf::parse_bool) == Some(true);
                            let enabled = doc.prop(n, "Enabled").and_then(wf::parse_bool) != Some(false);
                            let color = if enabled { 0x000000ff } else { 0x8a8a8aff };
                            let sub = !doc.items(n).is_empty();
                            place(local)
                                .flex()
                                .items_center()
                                .rounded(px(3.))
                                .when(self.selection.contains(n), |d| d.bg(rgba(0xe5f1fbff)))
                                .child(div().flex_none().w(px(28.)).flex().justify_center().when(checked, |d| d.child(text("✓".into(), color))))
                                .child(div().flex_1().min_w_0().overflow_hidden().child(text(strips::label(doc, n), color)))
                                .children(strips::shortcut(doc, n).map(|s| div().flex_none().pl(px(16.)).child(text(s, 0x6d6d6dff))))
                                .child(div().flex_none().w(px(18.)).flex().justify_center().when(sub, |d| d.child(text("›".into(), color))))
                                .into_any_element()
                        }
                    }
                })
                .collect();
            out.push(place(m.rect).bg(rgba(0xf9f9f9ff)).border_1().border_color(rgba(0xd0d0d0ff)).rounded(px(4.)).shadow(menu_shadow()).children(rows).into_any_element());
        }
        // Selected items get the selection's frame (controls get theirs from the layout).
        for (hit, r) in &self.item_hits {
            if let ItemHit::Item(n) = hit
                && self.selection.contains(n)
            {
                out.push(place(Rect::new(r.x - 1, r.y - 1, r.w + 2, r.h + 2)).border_1().border_dashed().border_color(selection).into_any_element());
            }
        }
        out
    }

    fn render_toolbox(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::global(cx);
        let fg = color(&t, "formDesigner.foreground", "sideBar.foreground");
        let muted = t.c("foreground.muted");
        let hover = t.c("list.hoverBackground");
        let active = t.c("list.activeSelectionBackground");
        let mut groups: Vec<(&'static str, Vec<&'static wf::ControlInfo>)> = Vec::new();
        for c in wf::TOOLBOX {
            match groups.iter_mut().find(|(g, _)| *g == c.group) {
                Some((_, v)) => v.push(c),
                None => groups.push((c.group, vec![c])),
            }
        }
        let tool = self.tool;
        let entry = |id: SharedString, label: &'static str, selected: bool| {
            div().id(id.clone()).debug_selector(move || id.to_string()).px_3().py(px(3.)).rounded(px(3.)).cursor_pointer().text_color(fg).when(selected, |d| d.bg(active)).hover(move |s| s.bg(hover)).child(label)
        };
        div()
            .id("designer-toolbox")
            .flex_none()
            .w(t.m("formDesigner.toolboxWidth", 170.0))
            .h_full()
            .overflow_y_scroll()
            .bg(t.bg("sideBar.background"))
            .border_r_1()
            .border_color(color(&t, "formDesigner.border", "editorWidget.border"))
            .text_size(t.m("ui.size", 13.0))
            .p_1()
            .child(div().px_2().py_1().text_size(px(11.)).text_color(muted).child("TOOLBOX"))
            .child(entry("designer-tool-pointer".into(), "Pointer", tool.is_none()).on_click(cx.listener(|this, _, _, cx| {
                this.tool = None;
                cx.notify();
            })))
            .children(groups.into_iter().map(|(group, items)| {
                div()
                    .child(div().px_2().pt_2().pb(px(2.)).text_size(px(11.)).text_color(muted).child(group))
                    .children(items.into_iter().map(|c| {
                        let ty = c.type_name;
                        entry(format!("designer-tool-{ty}").into(), ty, tool == Some(ty))
                            .on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                                if e.click_count() >= 2 {
                                    this.tool = None;
                                    this.add(ty, None, cx);
                                } else {
                                    this.tool = if this.tool == Some(ty) { None } else { Some(ty) };
                                }
                                cx.notify();
                            }))
                            .on_drag(DraggedTool(ty), |d, _, _, cx| cx.new(|_| d.clone()))
                    }))
            }))
    }

    fn render_form(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::global(cx);
        let Some(doc) = self.preview.clone().or_else(|| self.doc.clone()) else {
            let error = self.error.clone().unwrap_or_default();
            return div()
                .p_4()
                .flex()
                .flex_col()
                .gap_2()
                .text_color(color(&t, "formDesigner.foreground", "sideBar.foreground"))
                .child(div().child(format!("The designer can't read {}.", self.designer_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())))
                .child(div().text_color(t.c("foreground.muted")).child(error))
                .child(div().child("Fix the code (F7: View Code); the designer reads it again as you type."))
                .into_any_element();
        };
        self.layout = wf::layout(&doc, &self.tabs);
        let root = doc.root().name.clone();
        let is_form = doc.root().short_type() != "UserControl";
        let (w, h) = doc.size(&root);
        let back = rgba(doc.prop(&root, "BackColor").and_then(wf::parse_color).unwrap_or(0xf0f0f0ff));
        let border_style = doc.prop(&root, "FormBorderStyle").and_then(wf::parse_enum).unwrap_or_else(|| "Sizable".into());
        let framed = is_form && border_style != "None";
        let look = Look { doc: &doc, font_family: t.ui_font.clone() };
        let selection_color = color(&t, "formDesigner.selectionBorder", "focusBorder");
        let handle_bg = t.c_or("formDesigner.handleBackground", 0xffffffff);
        let handle_border = t.c_or("formDesigner.handleBorder", 0x000000ff);
        let hs = t.n("formDesigner.handleSize", 7.0) as i32;

        // Controls, back to front, each clipped by its container.
        let controls: Vec<AnyElement> = self
            .layout
            .controls
            .iter()
            .filter(|p| p.visible.w > 0 && p.visible.h > 0)
            .map(|p| {
                let inner = look.element(p, back, &self.tabs);
                let visible = doc.prop(&p.name, "Visible").and_then(wf::parse_bool) != Some(false);
                div()
                    .absolute()
                    .left(px(p.visible.x as f32))
                    .top(px(p.visible.y as f32))
                    .w(px(p.visible.w as f32))
                    .h(px(p.visible.h as f32))
                    .overflow_hidden()
                    .when(!visible, |d| d.opacity(0.5))
                    .child(div().absolute().left(px((p.rect.x - p.visible.x) as f32)).top(px((p.rect.y - p.visible.y) as f32)).w(px(p.rect.w as f32)).h(px(p.rect.h as f32)).child(inner))
                    .into_any_element()
            })
            .collect();
        // The items of strips, menus and grids: the menus the selection opens, the "Type Here" slots, and
        // where everything clickable is.
        let primary = self.primary().map(str::to_owned);
        let menus = primary.as_deref().map(|s| strips::open_menus(&doc, &self.layout, s)).unwrap_or_default();
        self.item_hits = strips::hits(&doc, &self.layout, primary.as_deref(), &menus);
        let item_overlays = self.render_item_overlays(&doc, &menus, selection_color, &t);
        let origin = self.origin.clone();
        let client = div()
            .id("designer-form-client")
            .debug_selector(|| "designer-form-client".into())
            .relative()
            .w(px(w as f32))
            .h(px(h as f32))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .overflow_hidden()
                    .bg(back)
                    .font_family(t.ui_font.clone())
                    .children(controls)
                    .child(gpui::canvas(move |bounds, _, _| origin.set(Some(bounds.origin)), |_, _, _, _| {}).absolute().inset_0()),
            )
            .children(item_overlays)
            // The selection and its handles draw over everything, outside the form too.
            .children(self.selection.iter().filter(|n| !doc.is_root(n)).filter_map(|n| self.layout.get(n)).map(|p| {
                let r = p.rect;
                div().absolute().left(px(r.x as f32 - 1.)).top(px(r.y as f32 - 1.)).w(px(r.w as f32 + 2.)).h(px(r.h as f32 + 2.)).border_1().border_dashed().border_color(selection_color)
            }))
            .children(self.handles(&doc, hs).into_iter().map(|(_, _, r, _)| {
                div().absolute().left(px(r.x as f32)).top(px(r.y as f32)).size(px(r.w as f32)).bg(handle_bg).border_1().border_color(handle_border)
            }))
            .children(self.guides.iter().map(|g| {
                let c = if g.margin { t.c_or("formDesigner.marginLine", 0xb000b0ff) } else { t.c_or("formDesigner.snapLine", 0x0078d4ff) };
                let (x, y) = (g.x0.min(g.x1), g.y0.min(g.y1));
                div().absolute().left(px(x as f32)).top(px(y as f32)).w(px((g.x1 - g.x0).abs().max(1) as f32)).h(px((g.y1 - g.y0).abs().max(1) as f32)).bg(c)
            }))
            .when_some(self.drag.as_ref().filter(|d| d.moved), |el, drag| {
                let accent = color(&t, "formDesigner.dropTargetBorder", "focusBorder");
                match &drag.kind {
                    DragKind::Create { .. } => {
                        let (x0, y0) = drag.start;
                        let (x1, y1) = drag.now;
                        el.child(
                            div()
                                .absolute()
                                .left(px(x0.min(x1) as f32))
                                .top(px(y0.min(y1) as f32))
                                .w(px((x1 - x0).abs() as f32))
                                .h(px((y1 - y0).abs() as f32))
                                .border_1()
                                .border_dashed()
                                .border_color(accent),
                        )
                    }
                    DragKind::Move { names } => {
                        let target = self.doc.as_ref().and_then(|d| self.layout.area_at(d, drag.now.0, drag.now.1, names));
                        let current = self.doc.as_ref().and_then(|d| names.first().and_then(|n| d.parent(n))).map(|(p, s)| (p.to_owned(), s.to_owned()));
                        match target.filter(|a| current.as_ref().is_none_or(|(p, s)| *p != a.owner || *s != a.slot)) {
                            Some(a) => el.child(
                                div()
                                    .absolute()
                                    .left(px(a.bounds.x as f32))
                                    .top(px(a.bounds.y as f32))
                                    .w(px(a.bounds.w as f32))
                                    .h(px(a.bounds.h as f32))
                                    .border_2()
                                    .border_color(accent),
                            ),
                            None => el,
                        }
                    }
                    _ => el,
                }
            });
        let root_selected = self.primary() == Some(root.as_str());
        let title = doc.text_of(&root).unwrap_or_default();
        let control_box = doc.prop(&root, "ControlBox").and_then(wf::parse_bool) != Some(false);
        let title_bar = framed.then(|| {
            div()
                .flex()
                .items_center()
                .h(px(TITLE_HEIGHT))
                .pl(px(10.))
                .bg(t.c_or("formDesigner.windowTitleBackground", 0xffffffff))
                .text_color(t.c_or("formDesigner.windowTitleForeground", 0x000000ff))
                .text_size(px(12.))
                .font_family(t.ui_font.clone())
                .child(div().size(px(14.)).mr(px(8.)).rounded(px(2.)).bg(rgba(0x5b9bd5ff)))
                .child(div().flex_1().min_w_0().whitespace_nowrap().overflow_hidden().child(title))
                .when(control_box, |d| d.children(["—", "☐", "✕"].map(|g| div().w(px(46.)).flex().justify_center().child(g))))
        });
        let frame = div()
            .flex_none()
            .flex()
            .flex_col()
            .when(framed, |d| d.border_1().border_color(t.c_or("formDesigner.windowBorder", 0x7a7a7aff)).shadow(t.shadow("widget.shadow")))
            .when(root_selected && !framed, |d| d.border_1().border_dashed().border_color(selection_color))
            .children(title_bar)
            .child(client);

        let tray = doc.tray();
        let tray_el = (!tray.is_empty()).then(|| {
            let fg = color(&t, "formDesigner.foreground", "sideBar.foreground");
            let hover = t.c("list.hoverBackground");
            let active = t.c("list.activeSelectionBackground");
            div()
                .mt_4()
                .p_2()
                .flex()
                .flex_wrap()
                .gap_2()
                .border_t_1()
                .border_color(color(&t, "formDesigner.border", "editorWidget.border"))
                .bg(color(&t, "formDesigner.trayBackground", "editor.background"))
                .text_color(fg)
                .text_size(t.m("ui.size", 13.0))
                .children(tray.into_iter().map(|c| {
                    let name = c.name.clone();
                    let selected = self.selection.contains(&name);
                    let label = format!("{}  ({})", c.name, c.short_type());
                    div()
                        .id(SharedString::from(format!("designer-tray-{name}")))
                        .debug_selector({
                            let name = name.clone();
                            move || format!("designer-tray-{name}")
                        })
                        .px_2()
                        .py_1()
                        .rounded(px(3.))
                        .cursor_pointer()
                        .when(selected, |d| d.bg(active))
                        .hover(move |s| s.bg(hover))
                        .child(label)
                        .on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                            this.select(&name, cx);
                            if e.click_count() >= 2 {
                                this.default_event(&name, cx);
                            }
                        }))
                }))
        });
        let _ = window;
        div()
            .p(px(MARGIN))
            .flex()
            .flex_col()
            .items_start()
            .child(frame)
            .children(tray_el)
            .into_any_element()
    }

    fn render_grid(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::global(cx);
        let fg = color(&t, "formDesigner.foreground", "sideBar.foreground");
        let muted = t.c("foreground.muted");
        let hover = t.c("list.hoverBackground");
        let line = color(&t, "formDesigner.border", "editorWidget.border");
        let row_h = t.m("formDesigner.rowHeight", 22.0);
        let doc = self.doc.clone();
        let owner = self.primary().map(str::to_owned);
        let mut body = div().flex().flex_col();
        let header;
        if let (Some(doc), Some(owner)) = (&doc, &owner) {
            let c = doc.get(owner);
            let ty = c.map(|c| c.type_name.clone()).unwrap_or_default();
            let count = self.selection.len();
            header = div()
                .id("designer-picker")
                .debug_selector(|| "designer-picker".into())
                .px_2()
                .py_1()
                .cursor_pointer()
                .border_b_1()
                .border_color(line)
                .hover(move |s| s.bg(hover))
                .child(div().text_color(fg).child(if count > 1 { format!("{owner} (+{} more)", count - 1) } else { owner.clone() }))
                .child(div().text_size(px(11.)).text_color(muted).child(ty))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.picker_open = !this.picker_open;
                    cx.notify();
                }));
            if self.picker_open {
                body = body.children(doc.components().iter().filter(|c| doc.is_root(&c.name) || doc.parent(&c.name).is_some() || doc.tray().iter().any(|t| t.name == c.name)).map(|c| {
                    let name = c.name.clone();
                    div()
                        .id(SharedString::from(format!("designer-pick-{name}")))
                        .px_2()
                        .h(row_h)
                        .flex()
                        .items_center()
                        .gap_2()
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover))
                        .child(div().text_color(fg).child(c.name.clone()))
                        .child(div().text_size(px(11.)).text_color(muted).child(c.short_type().to_owned()))
                        .on_click(cx.listener(move |this, _, _, cx| this.select(&name, cx)))
                }));
            }
            let tab = |id: &'static str, label: &'static str, on: bool| {
                div().id(id).debug_selector(move || id.into()).px_2().py(px(2.)).cursor_pointer().text_color(if on { fg } else { muted }).when(on, |d| d.border_b_2().border_color(t.c("focusBorder"))).child(label)
            };
            let tabs = div()
                .flex()
                .gap_1()
                .px_1()
                .border_b_1()
                .border_color(line)
                .child(tab("designer-tab-properties", "Properties", !self.show_events).on_click(cx.listener(|this, _, _, cx| {
                    this.show_events = false;
                    cx.notify();
                })))
                .child(tab("designer-tab-events", "Events", self.show_events).on_click(cx.listener(|this, _, _, cx| {
                    this.show_events = true;
                    cx.notify();
                })));
            body = body.child(tabs);
            let label_w = px(118.);
            if self.show_events {
                let ty = doc.get(owner).map(|c| c.type_name.clone()).unwrap_or_default();
                let hooked = doc.events(owner);
                for event in wf::events_for(&ty, doc.is_root(owner)) {
                    let handler = hooked.iter().find(|(e, _)| *e == event).map(|(_, h)| h.to_string());
                    let o = owner.clone();
                    let unhook = owner.clone();
                    body = body.child(
                        div()
                            .id(SharedString::from(format!("designer-event-{event}")))
                            .debug_selector(move || format!("designer-event-{event}"))
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
                                d.child(
                                    div()
                                        .id(SharedString::from(format!("designer-unhook-{event}")))
                                        .px_1()
                                        .text_color(muted)
                                        .cursor_pointer()
                                        .child("✕")
                                        .tooltip(crate::workbench::tooltip("Unhook (keeps the method)"))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            let o = unhook.clone();
                                            this.change(cx, |d| {
                                                d.set_event(&o, event, None);
                                                Ok(())
                                            });
                                        })),
                                )
                            })
                            .on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                                if e.click_count() >= 2 || this.doc.as_ref().is_some_and(|d| d.events(&o).iter().any(|(ev, _)| *ev == event)) {
                                    this.go_to_handler(&o, event, cx);
                                }
                            })),
                    );
                }
            } else {
                for (category, rows) in self.rows(doc, owner) {
                    body = body.child(div().px_2().pt_2().pb(px(2.)).text_size(px(11.)).text_color(muted).child(category));
                    for row in rows {
                        let value = Self::row_value(doc, owner, &row);
                        let label = row.label();
                        let editing = self.editing.as_ref() == Some(&row);
                        let id = format!("designer-prop-{}", label.trim_matches(['(', ')']));
                        let r = row.clone();
                        let value_el: AnyElement = if editing {
                            div().flex_1().min_w_0().child(self.input.clone()).into_any_element()
                        } else {
                            let swatch = matches!(&row, Row::Known(p) if p.kind == PropKind::Color).then(|| doc.prop(owner, row.label().as_str()).and_then(wf::parse_color)).flatten();
                            div()
                                .flex_1()
                                .min_w_0()
                                .px_2()
                                .flex()
                                .items_center()
                                .gap_1()
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_color(fg)
                                .when(value.set, |d| d.font_weight(gpui::FontWeight::BOLD))
                                .when_some(swatch, |d, c| d.child(div().flex_none().size(px(12.)).border_1().border_color(line).bg(rgba(c))))
                                .child(value.text.replace('\n', " "))
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
                                .on_click(cx.listener(move |this, _, w, cx| this.click_value(r.clone(), w, cx))),
                        );
                        if self.dropdown.as_ref() == Some(&row)
                            && let Row::Known(p) = row
                        {
                            body = body.child(self.render_dropdown(doc, owner, p, cx));
                        }
                    }
                }
            }
        } else {
            header = div().id("designer-picker");
        }
        let message = self.message.clone().map(|(m, error)| {
            div().p_2().border_t_1().border_color(line).text_color(if error { t.c_or("errorForeground", 0xf14c4cff) } else { muted }).child(m)
        });
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
            .child(div().id("designer-grid").flex_1().min_h_0().overflow_y_scroll().child(body))
            .children(message)
    }

    fn render_dropdown(&self, doc: &Designer, owner: &str, p: &'static PropInfo, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::global(cx);
        let fg = color(&t, "formDesigner.foreground", "sideBar.foreground");
        let hover = t.c("list.hoverBackground");
        let line = color(&t, "formDesigner.border", "editorWidget.border");
        let option = |id: String, label: String, value: String| {
            div()
                .id(SharedString::from(id.clone()))
                .debug_selector(move || id.clone())
                .pl(px(126.))
                .py(px(2.))
                .cursor_pointer()
                .text_color(fg)
                .hover(move |s| s.bg(hover))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| this.pick(p, value.clone(), cx)))
        };
        let list = div().py_1().bg(t.bg("menu.background")).border_b_1().border_color(line);
        match p.kind {
            PropKind::Enum(_, members) => list.children(members.iter().map(|m| option(format!("designer-option-{m}"), (*m).to_owned(), (*m).to_owned()))).into_any_element(),
            PropKind::Reference => {
                let names: Vec<String> = doc.components()[1..].iter().filter(|c| doc.parent(&c.name).is_some()).map(|c| c.name.clone()).collect();
                list.child(option("designer-option-none".into(), "(none)".into(), String::new()))
                    .children(names.into_iter().map(|n| option(format!("designer-option-{n}"), n.clone(), n)))
                    .into_any_element()
            }
            PropKind::Color => {
                const COLORS: &[&str] = &[
                    "SystemColors.Control", "SystemColors.ControlText", "SystemColors.Window", "SystemColors.WindowText", "SystemColors.Highlight", "Transparent", "White",
                    "Black", "Gray", "LightGray", "Red", "DarkRed", "Orange", "Gold", "Yellow", "Green", "LimeGreen", "Teal", "CornflowerBlue", "SteelBlue", "Blue", "Navy",
                    "Purple", "HotPink",
                ];
                let swatches = COLORS.iter().map(|name| {
                    let c = wf::format_color(doc.style, name).and_then(|code| wf::parse_color(&code)).unwrap_or(0);
                    let id = format!("designer-color-{name}");
                    let value = (*name).to_owned();
                    div()
                        .id(SharedString::from(id.clone()))
                        .debug_selector(move || id.clone())
                        .size(px(18.))
                        .border_1()
                        .border_color(line)
                        .bg(rgba(c))
                        .cursor_pointer()
                        .tooltip(crate::workbench::tooltip(*name))
                        .on_click(cx.listener(move |this, _, _, cx| this.pick(p, value.clone(), cx)))
                });
                let row = Row::Known(p);
                list.child(div().pl(px(126.)).pr_2().flex().flex_wrap().gap_1().children(swatches))
                    .child(option("designer-option-default".into(), "(default)".into(), String::new()))
                    .child(
                        div()
                            .id("designer-color-custom")
                            .pl(px(126.))
                            .py(px(2.))
                            .cursor_pointer()
                            .text_color(fg)
                            .hover(move |s| s.bg(hover))
                            .child("Custom… (#RRGGBB or r, g, b)")
                            .on_click(cx.listener(move |this, _, w, cx| this.start_edit(row.clone(), w, cx))),
                    )
                    .into_any_element()
            }
            PropKind::Anchor => {
                let current = doc.anchor(owner);
                let sides = ["Top", "Bottom", "Left", "Right"].map(|side| {
                    let on = current.iter().any(|a| a == side);
                    let mut next: Vec<String> = current.clone();
                    if on {
                        next.retain(|a| a != side);
                    } else {
                        next.push(side.to_owned());
                    }
                    let value = if next.is_empty() { "None".to_owned() } else { next.join(", ") };
                    let id = format!("designer-anchor-{side}");
                    div()
                        .id(SharedString::from(id.clone()))
                        .debug_selector(move || id.clone())
                        .px_2()
                        .py(px(2.))
                        .rounded(px(3.))
                        .border_1()
                        .border_color(line)
                        .cursor_pointer()
                        .text_color(fg)
                        .when(on, |d| d.bg(t.c("list.activeSelectionBackground")))
                        .child(side)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let Some(owner) = this.primary().map(str::to_owned) else { return };
                            let v = value.clone();
                            this.change(cx, |d| d.set_from_grid(&owner, p, &v));
                        }))
                });
                list.child(div().pl(px(126.)).pr_2().flex().flex_wrap().gap_1().children(sides)).into_any_element()
            }
            _ => div().into_any_element(),
        }
    }
}

impl Render for FormDesigner {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A grid field closed without another one opening: keys go to the designer again.
        if std::mem::take(&mut self.refocus) && self.editing.is_none() && self.input.focus_handle(cx).is_focused(window) {
            window.focus(&self.focus);
        }
        let t = Theme::global(cx);
        let fg = t.c("breadcrumb.foreground");
        let hover = t.c("toolbar.hoverBackground");
        let has_doc = self.doc.is_some();
        let selection_is_control = self.doc.as_ref().is_some_and(|d| self.selection.iter().any(|n| !d.is_root(n)));
        let several = self.doc.as_ref().is_some_and(|d| self.selection.iter().filter(|n| !d.is_root(n) && !d.is_item(n)).count() > 1);
        // An item moves among its owner's items (up and down in a menu, left and right on a strip or grid).
        let item = self.doc.as_ref().and_then(|d| {
            let n = self.primary()?;
            let owner = d.item_owner(n)?;
            Some(d.get(&owner).is_some_and(|c| c.short_type() == "ToolStripMenuItem"))
        });
        // A localized form: the languages it has, the one shown highlighted.
        let languages = self.resources.as_ref().filter(|_| self.doc.as_ref().is_some_and(Designer::is_localized)).map(|r| r.languages());
        let shown_language = self.language.clone();
        // A grid or one of its columns: columns of each type can be added.
        let grid = self.doc.as_ref().and_then(|d| {
            let n = self.primary()?;
            if d.get(n).is_some_and(|c| c.short_type() == "DataGridView") {
                return Some(n.to_owned());
            }
            d.item_owner(n).filter(|o| d.get(o).is_some_and(|c| c.short_type() == "DataGridView"))
        });
        let button = |id: &'static str, label: &'static str| {
            div().id(id).debug_selector(move || id.into()).px(px(6.)).py(px(1.)).rounded(px(4.)).cursor_pointer().hover(move |s| s.bg(hover)).child(label)
        };
        let name = self.code_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
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
            .when_some(self.tool, |d, tool| d.child(div().text_color(t.c("foreground.muted")).child(format!("Click or drag on the form to place a {tool} (Esc: cancel)"))))
            .when_some(languages, |d, languages| {
                let active = t.c("list.activeSelectionBackground");
                let choice = |id: String, label: String, language: Option<String>, on: bool| {
                    let sel = id.clone();
                    div()
                        .id(SharedString::from(id))
                        .debug_selector(move || sel.clone())
                        .px(px(6.))
                        .py(px(1.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .when(on, |d| d.bg(active))
                        .hover(move |s| s.bg(hover))
                        .child(label)
                        .on_click(cx.listener(move |this, _, _, cx| this.set_language(language.clone(), cx)))
                };
                d.child(div().text_color(t.c("foreground.muted")).child("Language:"))
                    .child(choice("designer-language-default".into(), "(Default)".into(), None, shown_language.is_none()))
                    .children(languages.into_iter().map(|l| {
                        let on = shown_language.as_deref() == Some(l.as_str());
                        choice(format!("designer-language-{l}"), l.clone(), Some(l), on)
                    }))
            })
            .child(div().flex_1())
            .when_some(grid, |d, grid| {
                d.child(div().text_color(t.c("foreground.muted")).child("Add Column:")).children(wf::COLUMN_TYPES.iter().map(|&(ty, label)| {
                    let grid = grid.clone();
                    let id = SharedString::from(format!("designer-add-column-{ty}"));
                    let sel = id.to_string();
                    div()
                        .id(id)
                        .debug_selector(move || sel.clone())
                        .px(px(6.))
                        .py(px(1.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover))
                        .child(label)
                        .on_click(cx.listener(move |this, _, window, cx| this.add_item(&grid, Some(ty), window, cx)))
                }))
            })
            .when_some(item, |d, vertical| {
                d.child(button("designer-item-back", if vertical { "Move Up" } else { "Move Left" }).on_click(cx.listener(|this, _, _, cx| this.move_item(-1, cx))))
                    .child(button("designer-item-forward", if vertical { "Move Down" } else { "Move Right" }).on_click(cx.listener(|this, _, _, cx| this.move_item(1, cx))))
            })
            .when(several, |d| {
                let aligns = [
                    ("designer-align-lefts", "Lefts", "Align the left edges with the primary control's", wf::Align::Lefts),
                    ("designer-align-centers", "Centers", "Align the centers with the primary control's", wf::Align::Centers),
                    ("designer-align-rights", "Rights", "Align the right edges with the primary control's", wf::Align::Rights),
                    ("designer-align-tops", "Tops", "Align the top edges with the primary control's", wf::Align::Tops),
                    ("designer-align-middles", "Middles", "Align the middles with the primary control's", wf::Align::Middles),
                    ("designer-align-bottoms", "Bottoms", "Align the bottom edges with the primary control's", wf::Align::Bottoms),
                    ("designer-same-size", "Same Size", "Give them the primary control's size", wf::Align::SameSize),
                ];
                d.child(div().text_color(t.c("foreground.muted")).child("Align:")).children(aligns.map(|(id, label, tip, how)| {
                    button(id, label).tooltip(crate::workbench::tooltip(tip)).on_click(cx.listener(move |this, _, _, cx| this.align(how, cx)))
                }))
            })
            .when(selection_is_control, |d| {
                // Items have no z-order: they move with Move Up / Down instead.
                d.when(item.is_none(), |d| {
                    d.child(button("designer-front", "Bring to Front").on_click(cx.listener(|this, _, _, cx| this.reorder(true, cx))))
                        .child(button("designer-back", "Send to Back").on_click(cx.listener(|this, _, _, cx| this.reorder(false, cx))))
                })
                .child(button("designer-delete", "Delete").on_click(cx.listener(|this, _, _, cx| this.delete_selection(cx))))
            })
            .child(button("designer-view-code", "View Code").on_click(cx.listener(|_, _, _, cx| cx.emit(DesignerEvent::ViewCode))));
        let form = self.render_form(window, cx);
        // The scrolled area is laid over its place, so a form wider than the window never pushes the grid
        // out of view.
        let scroller = div()
            .id("designer-canvas")
            .debug_selector(|| "designer-canvas".into())
            .absolute()
            .inset_0()
            .overflow_scroll()
            .bg(color(&t, "formDesigner.background", "editor.background"))
            .cursor(if self.tool.is_some() { gpui::CursorStyle::Crosshair } else { gpui::CursorStyle::Arrow })
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_drop(cx.listener(Self::on_drop_tool))
            .child(form);
        let canvas = div().relative().flex_1().min_w_0().h_full().child(scroller);
        div()
            .key_context("FormDesigner")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .font_family(t.ui_font.clone())
            .on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(|_, _: &ViewCode, _, cx| cx.emit(DesignerEvent::ViewCode)))
            .on_action(cx.listener(|this, _: &Undo, _, cx| this.undo_redo(false, cx)))
            .on_action(cx.listener(|this, _: &Redo, _, cx| this.undo_redo(true, cx)))
            .on_action(cx.listener(|this, _: &Copy, _, cx| {
                this.copy_selection(cx);
            }))
            .on_action(cx.listener(|this, _: &Cut, _, cx| this.cut_selection(cx)))
            .on_action(cx.listener(|this, _: &Paste, _, cx| this.paste(cx)))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| this.select_all(cx)))
            .child(toolbar)
            .child(div().flex_1().min_h_0().flex().when(has_doc, |d| d.child(self.render_toolbox(cx))).child(canvas).when(has_doc, |d| d.child(self.render_grid(cx))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_resize_their_sides() {
        let r = Rect::new(10, 10, 100, 50);
        assert_eq!(resized(r, Handle(1, 1), 5, 7), Rect::new(10, 10, 105, 57));
        assert_eq!(resized(r, Handle(-1, -1), 5, 7), Rect::new(15, 17, 95, 43));
        assert_eq!(resized(r, Handle(-1, 0), 500, 0).w, 1, "never smaller than a pixel");
        assert_eq!(FormDesigner::handle_rect(r, Handle(1, 1), 7), Rect::new(107, 57, 7, 7));
    }

    #[test]
    fn today_is_a_date() {
        let long = today(true);
        assert!(long.contains(", ") && long.split(", ").count() == 3, "{long}");
        assert_eq!(today(false).split('/').count(), 3);
    }
}
