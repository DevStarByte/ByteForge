//! Language-server decorations drawn inside the text: highlights of the other uses of the symbol under the
//! caret (`textDocument/documentHighlight`) and inlay hints (inferred types and parameter names,
//! `textDocument/inlayHint`).
//!
//! Inlay hints take up columns on screen. Every column → x conversion (painting, the caret, popups, hit
//! testing) adds the width of the hints before that column, using [`EditorView::inlay_shift`].

use std::time::Duration;

use bf_text::Point;
use gpui::Context;

use super::{EditorEvent, EditorView};

/// An inlay hint at a buffer position. `text` includes the padding spaces; the hint is drawn before the
/// character at `at`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Inlay {
    pub at: Point,
    pub text: String,
    /// Chars of padding before the label (0 or 1), so the background only covers the label.
    pub pad_left: usize,
    pub label_len: usize,
}

/// A highlighted use of the symbol under the caret (`write`: assigned there).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WordHighlight {
    pub start: Point,
    pub end: Point,
    pub write: bool,
}

/// Moves a point over a text change (points inside the replaced text go to its start).
pub(crate) fn shift_point(p: Point, c: &bf_text::Change) -> Point {
    if p < c.old_end {
        if p < c.start { p } else { c.start }
    } else if p.line == c.old_end.line {
        Point::new(c.new_end.line, c.new_end.col + (p.col - c.old_end.col))
    } else {
        Point::new((p.line as isize + c.new_end.line as isize - c.old_end.line as isize).max(0) as usize, p.col)
    }
}

impl EditorView {
    /// The inlay hints on `line`.
    pub(crate) fn inlays_on(&self, line: usize) -> &[Inlay] {
        let start = self.inlays.partition_point(|h| h.at.line < line);
        let end = self.inlays.partition_point(|h| h.at.line <= line);
        &self.inlays[start..end]
    }

    /// Screen columns taken by inlay hints before column `col` of `line`. A hint at `col` itself counts
    /// when `inclusive` (it is drawn before that character; the caret at `col` sits before the hint).
    pub(crate) fn inlay_shift(&self, line: usize, col: usize, inclusive: bool) -> usize {
        self.inlays_on(line)
            .iter()
            .take_while(|h| h.at.col < col || (inclusive && h.at.col == col))
            .map(|h| h.text.chars().count())
            .sum()
    }

    /// The column at screen column `visual` (fractional, in character cells) of `line`, skipping hints.
    pub(crate) fn col_at_visual(&self, line: usize, visual: f32) -> usize {
        let text = self.state.buffer.line(line);
        if self.inlays_on(line).is_empty() {
            return bf_text::logical_col(&text, visual, self.state.tab_size);
        }
        let tab = self.state.tab_size;
        let mut v = 0usize;
        for (i, ch) in text.trim_end_matches(['\n', '\r']).chars().enumerate() {
            let next = if ch == '\t' { (v / tab + 1) * tab } else { v + 1 };
            let shift = self.inlay_shift(line, i, true);
            if visual < (v + shift) as f32 + (next - v) as f32 / 2.0 {
                return i;
            }
            v = next;
        }
        text.trim_end_matches(['\n', '\r']).chars().count()
    }

    /// Keeps hints and highlights on their text while edits happen (until the server's next answer).
    pub(crate) fn shift_decorations(&mut self, c: &bf_text::Change) {
        // Hints on edited lines would show stale text in the wrong place; drop them.
        let edited = c.start.line..=c.old_end.line;
        self.inlays.retain(|h| !edited.contains(&h.at.line));
        for h in &mut self.inlays {
            h.at = shift_point(h.at, c);
        }
        self.word_highlights.clear();
    }

    /// Asks for the uses of the symbol under the caret once the caret rests (only in language-server files).
    pub(crate) fn schedule_word_highlights(&mut self, cx: &mut Context<Self>) {
        let (Some(lsp), Some(path)) = (self.lsp.clone(), self.path.clone()) else { return };
        let caret = self.state.caret();
        if self.word_highlights.iter().any(|h| h.start <= caret && caret <= h.end) {
            return;
        }
        let version = self.state.buffer.version();
        let position = self.lsp_position(caret);
        self.highlight_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(250)).await;
            let rx = lsp.document_highlight(&path, position);
            let Ok(Ok(found)) = rx.recv_async().await else { return };
            let _ = this.update(cx, |view, cx| {
                if view.state.buffer.version() != version {
                    return;
                }
                let buffer = &view.state.buffer;
                let highlights: Vec<WordHighlight> = found
                    .iter()
                    .map(|h| WordHighlight {
                        start: buffer.utf16_to_point(h.range.start.line as usize, h.range.start.character as usize),
                        end: buffer.utf16_to_point(h.range.end.line as usize, h.range.end.character as usize),
                        write: h.write,
                    })
                    .collect();
                // A lone highlight is just the word itself: nothing to point out.
                view.word_highlights = if highlights.len() > 1 { highlights } else { Vec::new() };
                cx.notify();
            });
        }));
    }

    /// Re-reads the inlay hints of the whole file after `delay` (edits restart the wait).
    pub(crate) fn schedule_inlay_hints(&mut self, delay: Duration, cx: &mut Context<Self>) {
        if !self.inlay_hints_enabled {
            if !self.inlays.is_empty() {
                self.inlays.clear();
                cx.notify();
            }
            return;
        }
        let (Some(lsp), Some(path)) = (self.lsp.clone(), self.path.clone()) else { return };
        let version = self.state.buffer.version();
        let range = bf_lsp::Range {
            start: bf_lsp::Position { line: 0, character: 0 },
            end: bf_lsp::Position { line: self.state.buffer.line_count() as u32, character: 0 },
        };
        self.inlay_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let rx = lsp.inlay_hints(&path, range);
            let Ok(Ok(hints)) = rx.recv_async().await else { return };
            let _ = this.update(cx, |view, cx| {
                if view.state.buffer.version() != version {
                    return;
                }
                let buffer = &view.state.buffer;
                let mut inlays: Vec<Inlay> = hints
                    .into_iter()
                    .map(|h| {
                        let at = buffer.utf16_to_point(h.position.line as usize, h.position.character as usize);
                        let pad_left = usize::from(h.padding_left);
                        let label_len = h.label.chars().count();
                        let text = format!("{}{}{}", if h.padding_left { " " } else { "" }, h.label, if h.padding_right { " " } else { "" });
                        Inlay { at, text, pad_left, label_len }
                    })
                    .collect();
                inlays.sort_by_key(|h| h.at);
                if inlays != view.inlays {
                    view.inlays = inlays;
                    view.line_cache.clear();
                    cx.emit(EditorEvent::CaretMoved);
                    cx.notify();
                }
            });
        }));
    }

    /// Turns inlay hints on or off (`"editor.inlayHints"`).
    pub fn set_inlay_hints(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.inlay_hints_enabled != enabled {
            self.inlay_hints_enabled = enabled;
            self.schedule_inlay_hints(Duration::ZERO, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_follow_edits() {
        let change = |start: (usize, usize), old_end: (usize, usize), new_end: (usize, usize)| bf_text::Change {
            start: Point::new(start.0, start.1),
            old_end: Point::new(old_end.0, old_end.1),
            new_end: Point::new(new_end.0, new_end.1),
            start_byte: 0,
            old_end_byte: 0,
            new_end_byte: 0,
            start_byte_point: (0, 0),
            old_end_byte_point: (0, 0),
            new_end_byte_point: (0, 0),
            start_utf16: (0, 0),
            old_end_utf16: (0, 0),
            removed: String::new(),
            inserted: String::new(),
        };
        // Typing 3 chars at 2:4.
        let c = change((2, 4), (2, 4), (2, 7));
        assert_eq!(shift_point(Point::new(2, 10), &c), Point::new(2, 13));
        assert_eq!(shift_point(Point::new(2, 1), &c), Point::new(2, 1));
        assert_eq!(shift_point(Point::new(5, 1), &c), Point::new(5, 1));
        // Deleting lines 3..5.
        let c = change((3, 0), (5, 0), (3, 0));
        assert_eq!(shift_point(Point::new(4, 2), &c), Point::new(3, 0));
        assert_eq!(shift_point(Point::new(7, 2), &c), Point::new(5, 2));
    }
}
