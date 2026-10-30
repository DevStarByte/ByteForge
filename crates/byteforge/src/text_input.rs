//! Single-line, theme-styled text input (palette, search, find/replace, prompts).

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler, Entity, EntityInputHandler,
    EventEmitter, FocusHandle, Focusable, GlobalElementId, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PaintQuad, Pixels, Point, ShapedLine, SharedString, Style, TextRun, UTF16Selection, Window, div, fill, point, prelude::*,
    px, relative, size,
};

use crate::actions::*;
use crate::theme::Theme;

pub enum InputEvent {
    Changed,
    Confirmed,
    Cancelled,
    Up,
    Down,
    PageUp,
    PageDown,
}

pub struct TextInput {
    focus: FocusHandle,
    text: String,
    placeholder: SharedString,
    selected: Range<usize>,
    reversed: bool,
    marked: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    selecting: bool,
}

impl EventEmitter<InputEvent> for TextInput {}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TextInput {
    pub fn new(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            text: String::new(),
            placeholder: placeholder.into(),
            selected: 0..0,
            reversed: false,
            marked: None,
            last_layout: None,
            last_bounds: None,
            selecting: false,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.text = text.into().replace('\n', " ");
        self.selected = self.text.len()..self.text.len();
        self.marked = None;
        cx.emit(InputEvent::Changed);
        cx.notify();
    }

    pub fn select_all_text(&mut self, cx: &mut Context<Self>) {
        self.selected = 0..self.text.len();
        cx.notify();
    }

    fn cursor(&self) -> usize {
        if self.reversed { self.selected.start } else { self.selected.end }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected = offset..offset;
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.reversed { self.selected.start = offset } else { self.selected.end = offset }
        if self.selected.end < self.selected.start {
            self.reversed = !self.reversed;
            self.selected = self.selected.end..self.selected.start;
        }
        cx.notify();
    }

    fn prev(&self, offset: usize) -> usize {
        self.text[..offset].char_indices().next_back().map_or(0, |(i, _)| i)
    }

    fn next(&self, offset: usize) -> usize {
        self.text[offset..].chars().next().map_or(offset, |c| offset + c.len_utf8())
    }

    fn replace(&mut self, range: Range<usize>, new: &str, cx: &mut Context<Self>) {
        let new = new.replace(['\n', '\r'], " ");
        self.text.replace_range(range.clone(), &new);
        let caret = range.start + new.len();
        self.selected = caret..caret;
        self.marked = None;
        cx.emit(InputEvent::Changed);
        cx.notify();
    }

    fn backspace(&mut self, _: &InputBackspace, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            let c = self.cursor();
            self.select_to(self.prev(c), cx);
        }
        self.replace(self.selected.clone(), "", cx);
    }

    fn delete(&mut self, _: &InputDelete, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            let c = self.cursor();
            self.select_to(self.next(c), cx);
        }
        self.replace(self.selected.clone(), "", cx);
    }

    fn left(&mut self, _: &InputLeft, _: &mut Window, cx: &mut Context<Self>) {
        let target = if self.selected.is_empty() { self.prev(self.cursor()) } else { self.selected.start };
        self.move_to(target, cx);
    }

    fn right(&mut self, _: &InputRight, _: &mut Window, cx: &mut Context<Self>) {
        let target = if self.selected.is_empty() { self.next(self.cursor()) } else { self.selected.end };
        self.move_to(target, cx);
    }

    fn select_left(&mut self, _: &InputSelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.prev(self.cursor()), cx);
    }

    fn select_right(&mut self, _: &InputSelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next(self.cursor()), cx);
    }

    fn home(&mut self, _: &InputHome, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &InputEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.text.len(), cx);
    }

    fn select_all(&mut self, _: &InputSelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.select_all_text(cx);
    }

    fn copy(&mut self, _: &InputCopy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.text[self.selected.clone()].to_owned()));
        }
    }

    fn cut(&mut self, _: &InputCut, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.text[self.selected.clone()].to_owned()));
            self.replace(self.selected.clone(), "", cx);
        }
    }

    fn paste(&mut self, _: &InputPaste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace(self.selected.clone(), &text, cx);
        }
    }

    fn index_for(&self, position: Point<Pixels>) -> usize {
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref()) else { return 0 };
        if self.text.is_empty() {
            return 0;
        }
        line.closest_index_for_x(position.x - bounds.left()).min(self.text.len())
    }

    fn mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        self.selecting = true;
        let i = self.index_for(e.position);
        if e.modifiers.shift { self.select_to(i, cx) } else { self.move_to(i, cx) }
        if e.click_count >= 2 {
            self.select_all_text(cx);
        }
    }

    fn mouse_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.selecting {
            let i = self.index_for(e.position);
            self.select_to(i, cx);
        }
    }

    fn mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }

    fn utf16_to_utf8(&self, offset: usize) -> usize {
        let mut units = 0;
        for (i, c) in self.text.char_indices() {
            if units >= offset {
                return i;
            }
            units += c.len_utf16();
        }
        self.text.len()
    }

    fn utf8_to_utf16(&self, offset: usize) -> usize {
        self.text[..offset.min(self.text.len())].chars().map(char::len_utf16).sum()
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(&mut self, r: Range<usize>, actual: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        let range = self.utf16_to_utf8(r.start)..self.utf16_to_utf8(r.end);
        actual.replace(self.utf8_to_utf16(range.start)..self.utf8_to_utf16(range.end));
        Some(self.text[range].to_owned())
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: self.utf8_to_utf16(self.selected.start)..self.utf8_to_utf16(self.selected.end), reversed: self.reversed })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|r| self.utf8_to_utf16(r.start)..self.utf8_to_utf16(r.end))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(&mut self, r: Option<Range<usize>>, new: &str, _: &mut Window, cx: &mut Context<Self>) {
        let range = r.map(|r| self.utf16_to_utf8(r.start)..self.utf16_to_utf8(r.end)).or(self.marked.clone()).unwrap_or(self.selected.clone());
        self.replace(range, new, cx);
    }

    fn replace_and_mark_text_in_range(&mut self, r: Option<Range<usize>>, new: &str, _sel: Option<Range<usize>>, _: &mut Window, cx: &mut Context<Self>) {
        let range = r.map(|r| self.utf16_to_utf8(r.start)..self.utf16_to_utf8(r.end)).or(self.marked.clone()).unwrap_or(self.selected.clone());
        self.replace(range.clone(), new, cx);
        if !new.is_empty() {
            self.marked = Some(range.start..range.start + new.len());
        }
    }

    fn bounds_for_range(&mut self, r: Range<usize>, bounds: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        let layout = self.last_layout.as_ref()?;
        let range = self.utf16_to_utf8(r.start)..self.utf16_to_utf8(r.end);
        Some(Bounds::from_corners(
            point(bounds.left() + layout.x_for_index(range.start), bounds.top()),
            point(bounds.left() + layout.x_for_index(range.end), bounds.bottom()),
        ))
    }

    fn character_index_for_point(&mut self, p: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        let bounds = self.last_bounds?;
        let layout = self.last_layout.as_ref()?;
        let i = layout.index_for_x(p.x - bounds.left())?;
        Some(self.utf8_to_utf16(i))
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        let t = Theme::global(cx);
        let focused_border = t.c("focusBorder");
        div()
            .key_context("TextInput")
            .track_focus(&self.focus)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(|_, _: &Confirm, _, cx| cx.emit(InputEvent::Confirmed)))
            .on_action(cx.listener(|_, _: &InputCancel, _, cx| cx.emit(InputEvent::Cancelled)))
            .on_action(cx.listener(|_, _: &InputUp, _, cx| cx.emit(InputEvent::Up)))
            .on_action(cx.listener(|_, _: &InputDown, _, cx| cx.emit(InputEvent::Down)))
            .on_action(cx.listener(|_, _: &InputPageUp, _, cx| cx.emit(InputEvent::PageUp)))
            .on_action(cx.listener(|_, _: &InputPageDown, _, cx| cx.emit(InputEvent::PageDown)))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .w_full()
            .bg(t.bg("input.background"))
            .text_color(t.c("input.foreground"))
            .border_color(t.c("input.border"))
            .when(focused, |d| d.border_color(focused_border))
            .border(px(t.edges("input.borderThickness", 1.0).top.into()))
            .rounded(t.radius("input.radius", 4.0))
            .px(t.edges("input.padding", 6.0).left)
            .py(t.edges("input.padding", 4.0).top)
            .text_size(t.m("ui.size", 13.0))
            .line_height(t.m("ui.size", 13.0) * 1.5)
            .child(InputElement { input: cx.entity() })
    }
}

struct InputElement {
    input: Entity<TextInput>,
}

struct InputPrepaint {
    line: ShapedLine,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for InputElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for InputElement {
    type RequestLayoutState = ();
    type PrepaintState = InputPrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> InputPrepaint {
        let input = self.input.read(cx);
        let t = Theme::global(cx);
        let style = window.text_style();
        let (text, color) = if input.text.is_empty() {
            (input.placeholder.clone(), t.c("input.placeholderForeground"))
        } else {
            (SharedString::from(input.text.clone()), style.color)
        };
        let run = TextRun { len: text.len(), font: style.font(), color, background_color: None, underline: None, strikethrough: None };
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window.text_system().shape_line(text, font_size, &[run], None);
        let cursor_x = if input.text.is_empty() { px(0.) } else { line.x_for_index(input.cursor()) };
        let selection = (!input.selected.is_empty() && !input.text.is_empty()).then(|| {
            fill(
                Bounds::from_corners(
                    point(bounds.left() + line.x_for_index(input.selected.start), bounds.top()),
                    point(bounds.left() + line.x_for_index(input.selected.end), bounds.bottom()),
                ),
                t.c("editor.selectionBackground"),
            )
        });
        let cursor = Some(fill(Bounds::new(point(bounds.left() + cursor_x, bounds.top()), size(px(1.5), bounds.size.height)), t.c("editorCursor.foreground")));
        InputPrepaint { line, cursor, selection }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut InputPrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.input.read(cx).focus.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.input.clone()), cx);
        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection);
        }
        let _ = prepaint.line.paint(bounds.origin, window.line_height(), window, cx);
        if focus.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }
        let line = prepaint.line.clone();
        self.input.update(cx, |input, _| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
        });
    }
}
