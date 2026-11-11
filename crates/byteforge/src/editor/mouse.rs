//! Mouse and scroll-wheel input.

use super::*;

impl EditorView {
    pub(super) fn point_at(&self, x: f32, y: f32) -> Point {
        let l = &self.layout;
        let lh = l.line_height.max(1.0);
        let row = (((y - l.top) + self.scroll.1) / lh).floor().max(0.0) as usize;
        let line = self.folds.line_of(row).min(self.state.buffer.line_count() - 1);
        let visual = (x - l.text_left + self.scroll.0) / l.char_width.max(1.0);
        Point::new(line, self.col_at_visual(line, visual))
    }

    pub(super) fn scrollbar_zone(&self, x: f32, y: f32) -> Option<bool> {
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

    pub(super) fn on_mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        self.completion = None;
        self.action_menu = None;
        self.context_menu = None;
        self.clear_hover(cx);
        let (x, y) = (f32::from(e.position.x), f32::from(e.position.y));
        if self.in_minimap(x, y) {
            self.minimap_scroll_to(y);
            self.drag = DragMode::Minimap;
            cx.notify();
            return;
        }
        if let Some(vertical) = self.scrollbar_zone(x, y) {
            self.drag = if vertical { DragMode::VScroll(y) } else { DragMode::HScroll(x) };
            return;
        }
        let p = self.point_at(x, y);
        if self.detect_links
            && (self.read_only || e.modifiers.secondary())
            && e.click_count == 1
            && x >= self.layout.text_left
            && let Some(link) = self.link_at(p)
        {
            let at = Point::new(link.line.saturating_sub(1) as usize, link.column.saturating_sub(1) as usize);
            cx.emit(EditorEvent::Navigate(link.path.clone(), at));
            return;
        }
        if e.modifiers.secondary() && !e.modifiers.shift {
            self.state.set_caret(p, false);
            self.after_move(cx);
            self.go_to_definition_at(p, cx);
            return;
        }
        if self.layout.fold_width > 0.0 && x >= self.layout.fold_left && x < self.layout.fold_left + self.layout.fold_width && y < self.layout.top + self.layout.height {
            self.toggle_fold(p.line, cx);
            return;
        }
        if self.glyph_margin && x < self.layout.left + self.layout.glyph && y < self.layout.top + self.layout.height {
            // A test marker runs its test (F9 still sets a breakpoint on that line).
            if self.test_marks.iter().any(|m| m.0 == p.line) && !self.breakpoints.iter().any(|b| b.line == p.line) {
                cx.emit(EditorEvent::RunTestAt(p.line));
                return;
            }
            self.toggle_breakpoint(p.line, cx);
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
        } else if e.modifiers.alt && !e.modifiers.shift {
            self.state.toggle_cursor(p);
            self.drag = DragMode::None;
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

    pub(super) fn on_mouse_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let (x, y) = (f32::from(e.position.x), f32::from(e.position.y));
        match self.drag {
            DragMode::None => {
                let over_minimap = self.in_minimap(x, y);
                if over_minimap != self.minimap_hover {
                    self.minimap_hover = over_minimap;
                    cx.notify();
                }
                if over_minimap {
                    self.clear_hover(cx);
                    return;
                }
                if self.hover.is_some() && self.hover_bounds.get().is_some_and(|b| b.contains(&e.position)) {
                    return; // reading the popup
                }
                // Hover: diagnostics immediately, language-server hover after a delay.
                let inside = x > self.layout.text_left && y > self.layout.top && y < self.layout.top + self.layout.height;
                if inside && e.pressed_button.is_none() {
                    let p = self.point_at(x, y);
                    // Past the end of a line there's nothing to hover.
                    if p.col < self.state.buffer.line_len(p.line) {
                        self.request_hover(p, cx);
                    } else {
                        self.clear_hover(cx);
                    }
                } else {
                    self.clear_hover(cx);
                }
                return;
            }
            DragMode::VScroll(last) => {
                let ratio = (self.folds.row_count(self.state.buffer.line_count()) as f32 * self.layout.line_height) / self.layout.height.max(1.0);
                self.scroll_to((self.scroll.0, self.scroll.1 + (y - last) * ratio.max(1.0)), false);
                self.drag = DragMode::VScroll(y);
                cx.notify();
                return;
            }
            DragMode::Minimap => {
                self.minimap_scroll_to(y);
                self.on_viewport_changed(cx);
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

    pub(super) fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.drag = DragMode::None;
    }

    pub(super) fn on_scroll(&mut self, e: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
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
}

impl EditorView {
    /// Right click: moves the caret there (unless it hit the selection) and opens the context menu.
    pub(super) fn on_right_mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        cx.stop_propagation();
        self.completion = None;
        self.action_menu = None;
        self.clear_hover(cx);
        let (x, y) = (f32::from(e.position.x), f32::from(e.position.y));
        if x >= self.layout.text_left && self.scrollbar_zone(x, y).is_none() && !self.in_minimap(x, y) {
            let p = self.point_at(x, y);
            let (start, end) = self.state.selection();
            if !(self.state.has_selection() && start <= p && p <= end) {
                self.state.set_caret(p, false);
                self.after_move(cx);
            }
        }
        self.context_menu = Some(ContextMenu { position: e.position });
        cx.notify();
    }
}
