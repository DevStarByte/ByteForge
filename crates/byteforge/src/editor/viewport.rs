//! Scrolling, revealing positions and file links.

use super::*;

impl EditorView {
    pub fn reveal(&mut self, p: Point, center: bool) {
        if self.layout.height <= 0.0 {
            self.pending_reveal = Some((p, center));
            return;
        }
        let lh = self.layout.line_height;
        let y = self.screen_pos(p, true).0 as f32 * lh;
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

    pub(super) fn ensure_caret_visible(&mut self) {
        if self.layout.height <= 0.0 {
            self.pending_reveal = Some((self.state.caret(), false));
            return;
        }
        let caret = self.state.caret();
        self.reveal(caret, false);
        if self.wrap.is_some() {
            return; // nothing runs off to the right
        }
        let line = self.state.buffer.line(caret.line);
        let x = (bf_text::visual_col(&line, caret.col, self.state.tab_size) + self.inlay_shift(caret.line, caret.col, false)) as f32 * self.layout.char_width;
        let width = self.layout.text_width();
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
        let max_y = ((self.row_count() as f32 - 1.0) * lh).max(0.0);
        let content_w = self.max_line_chars as f32 * self.layout.char_width + self.layout.char_width * 8.0;
        let view_w = self.layout.text_width();
        let max_x = if self.wrap.is_some() { 0.0 } else { (content_w - view_w).max(0.0) };
        (v.0.clamp(0.0, max_x), v.1.clamp(0.0, max_y))
    }

    pub(crate) fn scroll_to(&mut self, target: (f32, f32), animate: bool) {
        self.scroll_target = self.clamp_scroll(target);
        if !animate {
            self.scroll = self.scroll_target;
        }
        if let Some(link) = &self.scroll_link {
            let generation = link.get().2 + 1;
            link.set((self.scroll_target.1, animate, generation));
            self.scroll_link_seen = generation;
        }
    }

    /// Follows a linked editor's scrolling (called every frame).
    pub(crate) fn follow_scroll_link(&mut self) -> bool {
        let Some((y, animate, generation)) = self.scroll_link.as_ref().map(|l| l.get()) else { return false };
        if generation == self.scroll_link_seen {
            return false;
        }
        self.scroll_link_seen = generation;
        self.scroll_target.1 = y;
        if !animate {
            self.scroll.1 = y;
        }
        true
    }

    // ───────────── hit testing ─────────────

    /// The file references in `line` (cached by its text).
    pub(crate) fn links_in(&mut self, line: usize) -> Arc<Vec<bf_core::links::FileLink>> {
        let text = self.state.buffer.line(line).into_owned();
        if let Some(links) = self.link_cache.get(&text) {
            return links.clone();
        }
        if self.link_cache.len() > 4000 {
            self.link_cache.clear();
        }
        let links = Arc::new(bf_core::links::find_links(&text, self.link_root.as_deref()));
        self.link_cache.insert(text, links.clone());
        links
    }

    pub(super) fn link_at(&mut self, p: Point) -> Option<bf_core::links::FileLink> {
        let col = p.col;
        self.links_in(p.line).iter().find(|l| l.start <= col && col < l.end).cloned()
    }
}
