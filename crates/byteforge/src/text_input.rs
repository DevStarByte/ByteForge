//! Theme-styled text input: single-line (palette, search, find/replace, prompts), or multi-line with
//! wrapping (the commit message), where Enter starts a new line and Ctrl+Enter confirms.

use std::ops::Range;

use gpui::{
    App, AvailableSpace, Bounds, ClipboardItem, ContentMask, Context, CursorStyle, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, Font, GlobalElementId, Hsla, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, ShapedLine, SharedString, Style, TextAlign, TextRun, UTF16Selection, Window,
    WrappedLine, div, fill, point, prelude::*, px, relative, size,
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
    /// How far the text is scrolled to the left, so the caret stays in view when the text is wider than the box.
    scroll_x: Pixels,
    selecting: bool,
    /// Multi-line: at most this many rows show before the text scrolls.
    max_lines: Option<usize>,
    /// Multi-line: the text's lines (split at '\n') as last drawn, each wrapped to the box.
    paragraphs: Vec<Paragraph>,
    line_height: Pixels,
    /// Multi-line: how far the text is scrolled up.
    scroll_y: Pixels,
}

#[derive(Clone)]
/// One line of a multi-line input's text (up to a '\n'), wrapped into rows.
struct Paragraph {
    /// Byte offset of its first character in the text.
    start: usize,
    /// Its top, relative to the top of the text.
    top: Pixels,
    line: WrappedLine,
}

impl Paragraph {
    fn rows(&self) -> usize {
        self.line.wrap_boundaries().len() + 1
    }

    /// Each row's first byte (in this line) and the x where that row starts in the unwrapped line.
    fn row_starts(&self) -> Vec<(usize, Pixels)> {
        let layout = &self.line.unwrapped_layout;
        std::iter::once((0, px(0.)))
            .chain(self.line.wrap_boundaries().iter().map(|b| {
                let glyph = &layout.runs[b.run_ix].glyphs[b.glyph_ix];
                (glyph.index, glyph.position.x)
            }))
            .collect()
    }

    /// Where byte `local` of this line is drawn, relative to the top left of the text. At a wrap the
    /// position is the start of the next row.
    fn position(&self, local: usize, line_height: Pixels) -> Point<Pixels> {
        let rows = self.row_starts();
        let row = rows.iter().rposition(|(i, _)| *i <= local).unwrap_or(0);
        let x = self.line.unwrapped_layout.x_for_index(local) - rows[row].1;
        point(x, self.top + line_height * row as f32)
    }
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
            scroll_x: px(0.),
            selecting: false,
            max_lines: None,
            paragraphs: Vec::new(),
            line_height: px(0.),
            scroll_y: px(0.),
        }
    }

    /// A multi-line input: the text wraps, the box grows up to `max_lines` rows (then scrolls), Enter
    /// starts a new line and Ctrl+Enter (Cmd+Enter on macOS) confirms.
    pub fn multiline(placeholder: impl Into<SharedString>, max_lines: usize, cx: &mut Context<Self>) -> Self {
        Self { max_lines: Some(max_lines.max(1)), ..Self::new(placeholder, cx) }
    }

    fn is_multiline(&self) -> bool {
        self.max_lines.is_some()
    }

    /// Text as this input holds it: line breaks become spaces in a single-line input, and '\n' in a
    /// multi-line one.
    fn clean(&self, text: &str) -> String {
        if self.is_multiline() { text.replace("\r\n", "\n").replace('\r', "\n") } else { text.replace(['\n', '\r'], " ") }
    }

    /// Where the text was last drawn.
    #[cfg(test)]
    pub fn bounds_for_test(&self) -> Option<Bounds<Pixels>> {
        self.last_bounds
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.text = self.clean(&text.into());
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
        let new = self.clean(new);
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

    /// Home: the start of the text, or of the caret's line in a multi-line input.
    fn home(&mut self, _: &InputHome, _: &mut Window, cx: &mut Context<Self>) {
        let c = self.cursor();
        let target = if self.is_multiline() { self.text[..c].rfind('\n').map_or(0, |i| i + 1) } else { 0 };
        self.move_to(target, cx);
    }

    /// End: the end of the text, or of the caret's line in a multi-line input.
    fn end(&mut self, _: &InputEnd, _: &mut Window, cx: &mut Context<Self>) {
        let c = self.cursor();
        let target = if self.is_multiline() { self.text[c..].find('\n').map_or(self.text.len(), |i| c + i) } else { self.text.len() };
        self.move_to(target, cx);
    }

    /// Enter: confirms a single-line input; starts a new line in a multi-line one.
    fn enter(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_multiline() {
            self.replace(self.selected.clone(), "\n", cx);
        } else {
            cx.emit(InputEvent::Confirmed);
        }
    }

    /// Up/Down: the row above or below in a multi-line input (the start or end of the text past the
    /// first or last row); otherwise the owner's (a list's selection).
    fn vertical(&mut self, rows: f32, cx: &mut Context<Self>) {
        if !self.is_multiline() {
            cx.emit(if rows < 0. { InputEvent::Up } else { InputEvent::Down });
            return;
        }
        let pos = self.position_for(self.cursor());
        let y = pos.y + self.line_height * rows + self.line_height / 2.;
        let target = if y < px(0.) {
            0
        } else if y >= self.text_height() {
            self.text.len()
        } else {
            self.index_at(point(pos.x, y))
        };
        self.move_to(target, cx);
    }

    /// Height of the wrapped text (multi-line).
    fn text_height(&self) -> Pixels {
        self.paragraphs.last().map_or(self.line_height, |p| p.top + self.line_height * p.rows() as f32)
    }

    /// Where `index` is drawn, relative to the top left of the (unscrolled) text (multi-line).
    fn position_for(&self, index: usize) -> Point<Pixels> {
        let Some(p) = self.paragraphs.iter().rev().find(|p| p.start <= index) else { return point(px(0.), px(0.)) };
        p.position((index - p.start).min(p.line.len()), self.line_height)
    }

    /// The text index closest to `pos`, relative to the top left of the (unscrolled) text (multi-line).
    fn index_at(&self, pos: Point<Pixels>) -> usize {
        if self.text.is_empty() {
            return 0;
        }
        let Some(p) = self.paragraphs.iter().rev().find(|p| p.top <= pos.y).or(self.paragraphs.first()) else { return 0 };
        let bottom = self.line_height * p.rows() as f32 - px(1.);
        let y = (pos.y - p.top).max(px(0.)).min(bottom);
        let (Ok(i) | Err(i)) = p.line.closest_index_for_position(point(pos.x.max(px(0.)), y), self.line_height);
        (p.start + i).min(self.text.len())
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
        if self.is_multiline() {
            let Some(bounds) = self.last_bounds else { return 0 };
            return self.index_at(point(position.x - bounds.left(), position.y - bounds.top() + self.scroll_y));
        }
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref()) else { return 0 };
        if self.text.is_empty() {
            return 0;
        }
        line.closest_index_for_x(position.x - bounds.left() + self.scroll_x).min(self.text.len())
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
        let range = self.utf16_to_utf8(r.start)..self.utf16_to_utf8(r.end);
        if self.is_multiline() {
            let (start, end) = (self.position_for(range.start), self.position_for(range.end));
            let origin = point(bounds.left() + start.x, bounds.top() + start.y - self.scroll_y);
            let width = if end.y == start.y { end.x - start.x } else { px(0.) };
            return Some(Bounds::new(origin, size(width.max(px(1.)), self.line_height)));
        }
        let layout = self.last_layout.as_ref()?;
        Some(Bounds::from_corners(
            point(bounds.left() - self.scroll_x + layout.x_for_index(range.start), bounds.top()),
            point(bounds.left() - self.scroll_x + layout.x_for_index(range.end), bounds.bottom()),
        ))
    }

    fn character_index_for_point(&mut self, p: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        let bounds = self.last_bounds?;
        if self.is_multiline() {
            return Some(self.utf8_to_utf16(self.index_at(point(p.x - bounds.left(), p.y - bounds.top() + self.scroll_y))));
        }
        let layout = self.last_layout.as_ref()?;
        let i = layout.index_for_x(p.x - bounds.left() + self.scroll_x)?;
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
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(|_, _: &InputSubmit, _, cx| cx.emit(InputEvent::Confirmed)))
            .on_action(cx.listener(|_, _: &InputCancel, _, cx| cx.emit(InputEvent::Cancelled)))
            .on_action(cx.listener(|this, _: &InputUp, _, cx| this.vertical(-1., cx)))
            .on_action(cx.listener(|this, _: &InputDown, _, cx| this.vertical(1., cx)))
            .on_action(cx.listener(|_, _: &InputPageUp, _, cx| cx.emit(InputEvent::PageUp)))
            .on_action(cx.listener(|_, _: &InputPageDown, _, cx| cx.emit(InputEvent::PageDown)))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .w_full()
            .overflow_hidden()
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
            .map(|d| if self.is_multiline() { d.child(MultiLineElement { input: cx.entity() }) } else { d.child(InputElement { input: cx.entity() }) })
    }
}

struct InputElement {
    input: Entity<TextInput>,
}

struct InputPrepaint {
    line: ShapedLine,
    scroll_x: Pixels,
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
        let scroll_x = scroll_into_view(input.scroll_x, cursor_x, line.width, bounds.size.width);
        let left = bounds.left() - scroll_x;
        let selection = (!input.selected.is_empty() && !input.text.is_empty()).then(|| {
            fill(
                Bounds::from_corners(
                    point(left + line.x_for_index(input.selected.start), bounds.top()),
                    point(left + line.x_for_index(input.selected.end), bounds.bottom()),
                ),
                t.c("editor.selectionBackground"),
            )
        });
        let cursor = Some(fill(Bounds::new(point(left + cursor_x, bounds.top()), size(px(1.5), bounds.size.height)), t.c("editorCursor.foreground")));
        InputPrepaint { line, scroll_x, cursor, selection }
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
        let scroll_x = prepaint.scroll_x;
        // Text is clipped to the box; the caret may still show on its right edge.
        let mut clip = bounds;
        clip.size.width += px(2.);
        window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
            if let Some(selection) = prepaint.selection.take() {
                window.paint_quad(selection);
            }
            let _ = prepaint.line.paint(point(bounds.left() - scroll_x, bounds.top()), window.line_height(), window, cx);
            if focus.is_focused(window)
                && let Some(cursor) = prepaint.cursor.take()
            {
                window.paint_quad(cursor);
            }
        });
        let line = prepaint.line.clone();
        self.input.update(cx, |input, _| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
            input.scroll_x = scroll_x;
        });
    }
}

/// Shapes `text` into paragraphs (one per '\n'-separated line), each wrapped to `wrap`.
fn shape_paragraphs(text: &str, font: &Font, font_size: Pixels, color: Hsla, wrap: Option<Pixels>, line_height: Pixels, window: &Window) -> Vec<Paragraph> {
    let mut out = Vec::new();
    let (mut start, mut top) = (0, px(0.));
    for segment in text.split('\n') {
        let run = TextRun { len: segment.len(), font: font.clone(), color, background_color: None, underline: None, strikethrough: None };
        let line = window
            .text_system()
            .shape_text(SharedString::from(segment.to_owned()), font_size, &[run], wrap, None)
            .ok()
            .and_then(|lines| lines.into_iter().next())
            .unwrap_or_default();
        let paragraph = Paragraph { start, top, line };
        top += line_height * paragraph.rows() as f32;
        start += segment.len() + 1;
        out.push(paragraph);
    }
    out
}

/// The multi-line text: wrapped rows, as many as the text needs up to `max_lines`, scrolled to the caret.
struct MultiLineElement {
    input: Entity<TextInput>,
}

struct MultiLinePrepaint {
    paragraphs: Vec<Paragraph>,
    scroll_y: Pixels,
    selections: Vec<PaintQuad>,
    cursor: Option<PaintQuad>,
}

impl IntoElement for MultiLineElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for MultiLineElement {
    type RequestLayoutState = ();
    type PrepaintState = MultiLinePrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let input = self.input.read(cx);
        let shown = if input.text.is_empty() { input.placeholder.to_string() } else { input.text.clone() };
        let max_rows = input.max_lines.unwrap_or(1);
        let text_style = window.text_style();
        let font = text_style.font();
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let id = window.request_measured_layout(style, move |known, available, window, _| {
            let width = known.width.or(match available.width {
                AvailableSpace::Definite(w) => Some(w),
                _ => None,
            });
            let rows: usize = shape_paragraphs(&shown, &font, font_size, Hsla::default(), width, line_height, window).iter().map(Paragraph::rows).sum();
            size(width.unwrap_or_default(), line_height * rows.clamp(1, max_rows) as f32)
        });
        (id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> MultiLinePrepaint {
        let t = Theme::global(cx);
        let text_style = window.text_style();
        let font = text_style.font();
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();
        let wrap = Some(bounds.size.width);
        let (text, placeholder) = {
            let input = self.input.read(cx);
            (input.text.clone(), input.placeholder.clone())
        };
        let paragraphs = shape_paragraphs(&text, &font, font_size, text_style.color, wrap, line_height, window);
        let shown = if text.is_empty() {
            shape_paragraphs(&placeholder, &font, font_size, t.c("input.placeholderForeground"), wrap, line_height, window)
        } else {
            paragraphs.clone()
        };
        self.input.update(cx, |input, _| {
            input.paragraphs = paragraphs;
            input.line_height = line_height;
        });
        let input = self.input.read(cx);
        let caret = input.position_for(input.cursor());
        let height = bounds.size.height;
        let mut scroll_y = input.scroll_y;
        if caret.y < scroll_y {
            scroll_y = caret.y;
        } else if caret.y + line_height > scroll_y + height {
            scroll_y = caret.y + line_height - height;
        }
        let scroll_y = scroll_y.min((input.text_height() - height).max(px(0.))).max(px(0.));
        let origin = point(bounds.left(), bounds.top() - scroll_y);

        let mut selections = Vec::new();
        let sel = input.selected.clone();
        if !sel.is_empty() {
            let color = t.c("editor.selectionBackground");
            for p in &input.paragraphs {
                let rows = p.row_starts();
                for (r, &(row_start, row_x)) in rows.iter().enumerate() {
                    let row_end = rows.get(r + 1).map_or(p.line.len(), |(i, _)| *i);
                    let (a, b) = (p.start + row_start, p.start + row_end);
                    let (s, e) = (sel.start.max(a), sel.end.min(b));
                    // The selection goes on past this row (to the next one, or over the line break).
                    let continues = sel.end > b;
                    if s > e || (s == e && !continues) {
                        continue;
                    }
                    let layout = &p.line.unwrapped_layout;
                    let x0 = layout.x_for_index(s - p.start) - row_x;
                    let x1 = layout.x_for_index(e - p.start) - row_x + if continues { px(4.) } else { px(0.) };
                    let y = p.top + line_height * r as f32;
                    selections.push(fill(Bounds::from_corners(point(origin.x + x0, origin.y + y), point(origin.x + x1, origin.y + y + line_height)), color));
                }
            }
        }
        let cursor = Some(fill(Bounds::new(point(origin.x + caret.x, origin.y + caret.y), size(px(1.5), line_height)), t.c("editorCursor.foreground")));
        MultiLinePrepaint { paragraphs: shown, scroll_y, selections, cursor }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut MultiLinePrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.input.read(cx).focus.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.input.clone()), cx);
        let scroll_y = prepaint.scroll_y;
        let line_height = window.line_height();
        // Text is clipped to the box; the caret may still show on its right edge.
        let mut clip = bounds;
        clip.size.width += px(2.);
        window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
            for selection in prepaint.selections.drain(..) {
                window.paint_quad(selection);
            }
            for p in &prepaint.paragraphs {
                let _ = p.line.paint(point(bounds.left(), bounds.top() + p.top - scroll_y), line_height, TextAlign::Left, None, window, cx);
            }
            if focus.is_focused(window)
                && let Some(cursor) = prepaint.cursor.take()
            {
                window.paint_quad(cursor);
            }
        });
        self.input.update(cx, |input, _| {
            input.last_bounds = Some(bounds);
            input.scroll_y = scroll_y;
        });
    }
}

/// The scroll offset that keeps the caret (at `cursor_x` in a line `line_width` wide) inside a box `width` wide:
/// unchanged while the caret is visible, no further than the end of the text, never negative.
fn scroll_into_view(scroll: Pixels, cursor_x: Pixels, line_width: Pixels, width: Pixels) -> Pixels {
    let mut scroll = scroll;
    if cursor_x < scroll {
        scroll = cursor_x;
    } else if cursor_x > scroll + width {
        scroll = cursor_x - width;
    }
    scroll.min((line_width - width).max(px(0.))).max(px(0.))
}

#[cfg(test)]
mod tests {
    use super::scroll_into_view;
    use gpui::px;

    #[test]
    fn the_caret_stays_in_view() {
        // short text: no scrolling
        assert_eq!(scroll_into_view(px(0.), px(50.), px(80.), px(100.)), px(0.));
        // the caret at the end of a long text: scrolled so it sits on the right edge
        assert_eq!(scroll_into_view(px(0.), px(300.), px(300.), px(100.)), px(200.));
        // moving left inside the visible part keeps the offset; past it, the text follows
        assert_eq!(scroll_into_view(px(200.), px(250.), px(300.), px(100.)), px(200.));
        assert_eq!(scroll_into_view(px(200.), px(120.), px(300.), px(100.)), px(120.));
        // text deleted: never scrolled past its end
        assert_eq!(scroll_into_view(px(200.), px(30.), px(30.), px(100.)), px(0.));
    }
}
