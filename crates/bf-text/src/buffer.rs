use std::borrow::Cow;

use ropey::Rope;

use crate::undo::UndoStack;

/// A position in the buffer: zero-based line and column counted in `char`s.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Point {
    pub line: usize,
    pub col: usize,
}

impl Point {
    pub const fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
        }
    }
}

/// One applied edit, described in every coordinate system a consumer needs:
/// chars/points for views, bytes + byte-points for tree-sitter, UTF-16 for LSP.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub start: Point,
    pub old_end: Point,
    pub new_end: Point,
    pub start_byte: usize,
    pub old_end_byte: usize,
    pub new_end_byte: usize,
    /// (row, byte column) as tree-sitter expects.
    pub start_byte_point: (usize, usize),
    pub old_end_byte_point: (usize, usize),
    pub new_end_byte_point: (usize, usize),
    /// LSP range of the replaced text in the *old* document (line, UTF-16 column).
    pub start_utf16: (usize, usize),
    pub old_end_utf16: (usize, usize),
    pub removed: String,
    pub inserted: String,
}

/// Rope-backed text with normalized `\n` newlines, a version counter and undo history.
/// Edits are O(log n); the original line ending is restored by [`Buffer::text_for_save`].
pub struct Buffer {
    rope: Rope,
    version: u64,
    line_ending: LineEnding,
    undo: UndoStack,
    saved_state: u64,
    changes: Vec<Change>,
}

impl Default for Buffer {
    fn default() -> Self {
        Self::new("")
    }
}

impl Buffer {
    pub fn new(text: &str) -> Self {
        let line_ending = if text.contains("\r\n") { LineEnding::CrLf } else { LineEnding::Lf };
        let normalized = normalize(text);
        Self {
            rope: Rope::from_str(&normalized),
            version: 0,
            line_ending,
            undo: UndoStack::default(),
            saved_state: 0,
            changes: Vec::new(),
        }
    }

    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn line_ending(&self) -> LineEnding {
        self.line_ending
    }

    pub fn set_line_ending(&mut self, ending: LineEnding) {
        self.line_ending = ending;
    }

    pub fn is_modified(&self) -> bool {
        self.undo.state_id() != self.saved_state
    }

    pub fn mark_saved(&mut self) {
        self.saved_state = self.undo.state_id();
    }

    pub fn undo_stack(&mut self) -> &mut UndoStack {
        &mut self.undo
    }

    /// Changes applied since the last call (for syntax highlighters and language servers).
    pub fn take_changes(&mut self) -> Vec<Change> {
        std::mem::take(&mut self.changes)
    }

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    pub fn text_for_save(&self) -> String {
        match self.line_ending {
            LineEnding::Lf => self.text(),
            LineEnding::CrLf => self.text().replace('\n', "\r\n"),
        }
    }

    pub fn len_bytes(&self) -> usize {
        self.rope.len_bytes()
    }

    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    /// Line text without the trailing newline.
    pub fn line(&self, line: usize) -> Cow<'_, str> {
        let slice = self.rope.line(line);
        let text: Cow<str> = slice.into();
        match text {
            Cow::Borrowed(s) => Cow::Borrowed(s.strip_suffix('\n').unwrap_or(s)),
            Cow::Owned(mut s) => {
                if s.ends_with('\n') {
                    s.pop();
                }
                Cow::Owned(s)
            }
        }
    }

    pub fn line_len(&self, line: usize) -> usize {
        let slice = self.rope.line(line);
        let len = slice.len_chars();
        if len > 0 && slice.char(len - 1) == '\n' { len - 1 } else { len }
    }

    pub fn end(&self) -> Point {
        let last = self.line_count() - 1;
        Point::new(last, self.line_len(last))
    }

    pub fn clamp(&self, p: Point) -> Point {
        let line = p.line.min(self.line_count() - 1);
        Point::new(line, p.col.min(self.line_len(line)))
    }

    pub fn point_to_char(&self, p: Point) -> usize {
        let p = self.clamp(p);
        self.rope.line_to_char(p.line) + p.col
    }

    pub fn char_to_point(&self, char_idx: usize) -> Point {
        let idx = char_idx.min(self.rope.len_chars());
        let line = self.rope.char_to_line(idx);
        Point::new(line, idx - self.rope.line_to_char(line))
    }

    pub fn point_to_byte(&self, p: Point) -> usize {
        self.rope.char_to_byte(self.point_to_char(p))
    }

    pub fn byte_to_point(&self, byte: usize) -> Point {
        self.char_to_point(self.rope.byte_to_char(byte.min(self.rope.len_bytes())))
    }

    /// (row, byte column) for tree-sitter.
    pub fn byte_point(&self, p: Point) -> (usize, usize) {
        let p = self.clamp(p);
        (p.line, self.point_to_byte(p) - self.rope.line_to_byte(p.line))
    }

    /// (line, UTF-16 column) for LSP.
    pub fn point_to_utf16(&self, p: Point) -> (usize, usize) {
        let p = self.clamp(p);
        let line = self.rope.line(p.line);
        let col = line.slice(..p.col).chars().map(char::len_utf16).sum();
        (p.line, col)
    }

    pub fn utf16_to_point(&self, line: usize, utf16_col: usize) -> Point {
        if line >= self.line_count() {
            return self.end();
        }
        let mut units = 0;
        let mut col = 0;
        for ch in self.rope.line(line).chars() {
            if units >= utf16_col || ch == '\n' {
                break;
            }
            units += ch.len_utf16();
            col += 1;
        }
        Point::new(line, col)
    }

    pub fn slice(&self, start: Point, end: Point) -> String {
        let (a, b) = order(self.clamp(start), self.clamp(end));
        self.rope.slice(self.point_to_char(a)..self.point_to_char(b)).to_string()
    }

    /// Replaces `[start, end)` with `text`, records undo and returns the end of the inserted text.
    pub fn replace(&mut self, start: Point, end: Point, text: &str) -> Point {
        self.replace_inner(start, end, text, true)
    }

    pub(crate) fn replace_inner(&mut self, start: Point, end: Point, text: &str, record: bool) -> Point {
        let (start, end) = order(self.clamp(start), self.clamp(end));
        let text = normalize(text);
        if start == end && text.is_empty() {
            return start;
        }
        let start_char = self.point_to_char(start);
        let end_char = self.point_to_char(end);
        let start_byte = self.rope.char_to_byte(start_char);
        let old_end_byte = self.rope.char_to_byte(end_char);
        let start_byte_point = self.byte_point(start);
        let old_end_byte_point = self.byte_point(end);
        let start_utf16 = self.point_to_utf16(start);
        let old_end_utf16 = self.point_to_utf16(end);
        let removed = self.rope.slice(start_char..end_char).to_string();

        self.rope.remove(start_char..end_char);
        self.rope.insert(start_char, &text);
        let new_end = self.char_to_point(start_char + text.chars().count());
        let new_end_byte = start_byte + text.len();

        self.version += 1;
        if record {
            self.undo.record(start, &removed, &text);
        }
        self.changes.push(Change {
            start,
            old_end: end,
            new_end,
            start_byte,
            old_end_byte,
            new_end_byte,
            start_byte_point,
            old_end_byte_point,
            new_end_byte_point: self.byte_point(new_end),
            start_utf16,
            old_end_utf16,
            removed,
            inserted: text.into_owned(),
        });
        new_end
    }

    /// Replaces the whole text (e.g. reload from disk); clears undo history.
    pub fn set_text(&mut self, text: &str) {
        let end = self.end();
        self.replace_inner(Point::default(), end, text, false);
        self.line_ending = if text.contains("\r\n") { LineEnding::CrLf } else { LineEnding::Lf };
        self.undo = UndoStack::default();
        self.saved_state = self.undo.state_id();
    }

    /// Undoes the last entry; returns the caret position to restore.
    pub fn undo(&mut self) -> Option<Point> {
        let entry = self.undo.pop_undo()?;
        let mut caret = None;
        for edit in entry.edits.iter().rev() {
            let end = end_of(edit.start, &edit.inserted);
            caret = Some(self.replace_inner(edit.start, end, &edit.removed, false));
        }
        self.undo.push_redo(entry);
        caret
    }

    pub fn redo(&mut self) -> Option<Point> {
        let entry = self.undo.pop_redo()?;
        let mut caret = None;
        for edit in &entry.edits {
            let end = end_of(edit.start, &edit.removed);
            caret = Some(self.replace_inner(edit.start, end, &edit.inserted, false));
        }
        self.undo.push_undo(entry);
        caret
    }
}

pub(crate) fn order(a: Point, b: Point) -> (Point, Point) {
    if a <= b { (a, b) } else { (b, a) }
}

/// Position after inserting `text` at `start`.
pub(crate) fn end_of(start: Point, text: &str) -> Point {
    match text.rfind('\n') {
        None => Point::new(start.line, start.col + text.chars().count()),
        Some(last) => Point::new(start.line + text.matches('\n').count(), text[last + 1..].chars().count()),
    }
}

fn normalize(text: &str) -> Cow<'_, str> {
    if text.contains('\r') { Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n")) } else { Cow::Borrowed(text) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_reports_all_coordinate_systems() {
        let mut b = Buffer::new("héllo\nwörld");
        let end = b.replace(Point::new(1, 1), Point::new(1, 2), "ØØ");
        assert_eq!(b.text(), "héllo\nwØØrld");
        assert_eq!(end, Point::new(1, 3));
        let c = &b.take_changes()[0];
        assert_eq!(c.start_byte, "héllo\nw".len());
        assert_eq!(c.old_end_byte, "héllo\nwö".len());
        assert_eq!(c.new_end_byte, "héllo\nwØØ".len());
        assert_eq!(c.start_byte_point, (1, 1));
        assert_eq!(c.start_utf16, (1, 1));
    }

    #[test]
    fn crlf_is_normalized_and_restored() {
        let b = Buffer::new("a\r\nb\r\n");
        assert_eq!(b.text(), "a\nb\n");
        assert_eq!(b.line_count(), 3);
        assert_eq!(b.text_for_save(), "a\r\nb\r\n");
    }

    #[test]
    fn undo_redo_and_modified_state() {
        let mut b = Buffer::new("abc");
        assert!(!b.is_modified());
        b.replace(Point::new(0, 3), Point::new(0, 3), "d");
        b.replace(Point::new(0, 4), Point::new(0, 4), "e");
        assert!(b.is_modified());
        assert_eq!(b.undo(), Some(Point::new(0, 3)));
        assert_eq!(b.text(), "abc");
        assert!(!b.is_modified());
        b.redo();
        assert_eq!(b.text(), "abcde");
    }

    #[test]
    fn utf16_round_trip() {
        let b = Buffer::new("a😀b");
        assert_eq!(b.point_to_utf16(Point::new(0, 2)), (0, 3));
        assert_eq!(b.utf16_to_point(0, 3), Point::new(0, 2));
    }

    #[test]
    fn multiline_insert_and_line_access() {
        let mut b = Buffer::new("one\ntwo");
        let end = b.replace(Point::new(0, 3), Point::new(0, 3), "\nmid\n");
        assert_eq!(end, Point::new(2, 0));
        assert_eq!(b.line(1), "mid");
        assert_eq!(b.line_len(3), 3);
        assert_eq!(b.end(), Point::new(3, 3));
    }
}
