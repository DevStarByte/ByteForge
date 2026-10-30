use crate::buffer::{Buffer, Point, order};

/// Character classes for word movement and double-click selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CharClass {
    Whitespace,
    Word,
    Other,
}

pub fn char_class(c: char) -> CharClass {
    if c.is_whitespace() {
        CharClass::Whitespace
    } else if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else {
        CharClass::Other
    }
}

/// Visual column of `col` in `line`, expanding tabs to `tab_size` stops.
pub fn visual_col(line: &str, col: usize, tab_size: usize) -> usize {
    let mut v = 0;
    let mut count = 0;
    for ch in line.chars().take(col) {
        v = if ch == '\t' { (v / tab_size + 1) * tab_size } else { v + 1 };
        count += 1;
    }
    v + col.saturating_sub(count)
}

/// Character column closest to the (fractional) visual column `visual`.
pub fn logical_col(line: &str, visual: f32, tab_size: usize) -> usize {
    if visual <= 0.0 {
        return 0;
    }
    let mut v = 0usize;
    for (i, ch) in line.chars().enumerate() {
        let next = if ch == '\t' { (v / tab_size + 1) * tab_size } else { v + 1 };
        if visual < (v + next) as f32 / 2.0 {
            return i;
        }
        v = next;
    }
    line.chars().count()
}

const PAIRS: [(char, char); 5] = [('(', ')'), ('[', ']'), ('{', '}'), ('"', '"'), ('\'', '\'')];

fn closer_of(c: char) -> Option<char> {
    PAIRS.iter().find(|(o, _)| *o == c).map(|(_, c)| *c)
}

/// Caret, selection and every editing command of the editor. All mutations go through the
/// [`Buffer`], which records undo and the change list consumed by highlighters and language servers.
pub struct EditorState {
    pub buffer: Buffer,
    caret: Point,
    anchor: Option<Point>,
    desired_visual_col: Option<usize>,
    pub tab_size: usize,
    pub insert_spaces: bool,
    pub auto_close: bool,
    pub read_only: bool,
    pub line_comment: Option<&'static str>,
}

impl EditorState {
    pub fn new(buffer: Buffer) -> Self {
        Self {
            buffer,
            caret: Point::default(),
            anchor: None,
            desired_visual_col: None,
            tab_size: 4,
            insert_spaces: true,
            auto_close: true,
            read_only: false,
            line_comment: None,
        }
    }

    // ───────────── caret & selection ─────────────

    pub fn caret(&self) -> Point {
        self.caret
    }

    pub fn anchor(&self) -> Option<Point> {
        self.anchor
    }

    pub fn has_selection(&self) -> bool {
        self.anchor.is_some_and(|a| a != self.caret)
    }

    /// Ordered selection (start, end); empty at the caret when nothing is selected.
    pub fn selection(&self) -> (Point, Point) {
        match self.anchor {
            Some(a) if a != self.caret => order(a, self.caret),
            _ => (self.caret, self.caret),
        }
    }

    pub fn selected_text(&self) -> String {
        let (s, e) = self.selection();
        self.buffer.slice(s, e)
    }

    pub fn set_caret(&mut self, p: Point, extend: bool) {
        let p = self.buffer.clamp(p);
        if extend {
            self.anchor.get_or_insert(self.caret);
        } else {
            self.anchor = None;
        }
        self.caret = p;
        self.desired_visual_col = None;
    }

    pub fn select(&mut self, anchor: Point, caret: Point) {
        self.anchor = Some(self.buffer.clamp(anchor));
        self.caret = self.buffer.clamp(caret);
        self.desired_visual_col = None;
    }

    pub fn select_all(&mut self) {
        let end = self.buffer.end();
        self.select(Point::default(), end);
    }

    pub fn clear_selection(&mut self) {
        self.anchor = None;
    }

    /// Re-clamps caret/anchor after external buffer changes.
    pub fn clamp(&mut self) {
        self.caret = self.buffer.clamp(self.caret);
        self.anchor = self.anchor.map(|a| self.buffer.clamp(a));
    }

    pub fn word_at(&self, p: Point) -> (Point, Point) {
        let line: Vec<char> = self.buffer.line(p.line).chars().collect();
        if line.is_empty() {
            return (p, p);
        }
        let mut col = p.col.min(line.len() - 1);
        if p.col > 0 && p.col == line.len() {
            col = p.col - 1;
        }
        let class = char_class(line[col]);
        let (mut s, mut e) = (col, col + 1);
        while s > 0 && char_class(line[s - 1]) == class {
            s -= 1;
        }
        while e < line.len() && char_class(line[e]) == class {
            e += 1;
        }
        (Point::new(p.line, s), Point::new(p.line, e))
    }

    // ───────────── movement ─────────────

    pub fn char_left(&self, p: Point) -> Point {
        if p.col > 0 {
            Point::new(p.line, p.col - 1)
        } else if p.line > 0 {
            Point::new(p.line - 1, self.buffer.line_len(p.line - 1))
        } else {
            p
        }
    }

    pub fn char_right(&self, p: Point) -> Point {
        if p.col < self.buffer.line_len(p.line) {
            Point::new(p.line, p.col + 1)
        } else if p.line + 1 < self.buffer.line_count() {
            Point::new(p.line + 1, 0)
        } else {
            p
        }
    }

    pub fn word_left(&self, p: Point) -> Point {
        if p.col == 0 {
            return self.char_left(p);
        }
        let line: Vec<char> = self.buffer.line(p.line).chars().collect();
        let mut i = p.col.min(line.len());
        while i > 0 && line[i - 1].is_whitespace() {
            i -= 1;
        }
        if i > 0 {
            let class = char_class(line[i - 1]);
            while i > 0 && char_class(line[i - 1]) == class {
                i -= 1;
            }
        }
        Point::new(p.line, i)
    }

    pub fn word_right(&self, p: Point) -> Point {
        let line: Vec<char> = self.buffer.line(p.line).chars().collect();
        if p.col >= line.len() {
            return self.char_right(p);
        }
        let mut i = p.col;
        let class = char_class(line[i]);
        while i < line.len() && char_class(line[i]) == class {
            i += 1;
        }
        while i < line.len() && line[i].is_whitespace() {
            i += 1;
        }
        Point::new(p.line, i)
    }

    pub fn smart_home(&self, p: Point) -> Point {
        let first = self.buffer.line(p.line).chars().take_while(|c| c.is_whitespace()).count();
        Point::new(p.line, if p.col == first { 0 } else { first })
    }

    pub fn line_end(&self, p: Point) -> Point {
        Point::new(p.line, self.buffer.line_len(p.line))
    }

    pub fn move_left(&mut self, word: bool, extend: bool) {
        if !extend && !word && self.has_selection() {
            let (s, _) = self.selection();
            self.set_caret(s, false);
        } else {
            let p = if word { self.word_left(self.caret) } else { self.char_left(self.caret) };
            self.set_caret(p, extend);
        }
    }

    pub fn move_right(&mut self, word: bool, extend: bool) {
        if !extend && !word && self.has_selection() {
            let (_, e) = self.selection();
            self.set_caret(e, false);
        } else {
            let p = if word { self.word_right(self.caret) } else { self.char_right(self.caret) };
            self.set_caret(p, extend);
        }
    }

    /// Vertical movement keeping the desired visual column (like every code editor).
    pub fn move_vertical(&mut self, delta: isize, extend: bool) {
        let line_text = self.buffer.line(self.caret.line);
        let desired = self.desired_visual_col.unwrap_or_else(|| visual_col(&line_text, self.caret.col, self.tab_size));
        let target = self.caret.line as isize + delta;
        let p = if target < 0 {
            Point::default()
        } else if target as usize >= self.buffer.line_count() {
            self.buffer.end()
        } else {
            let line = target as usize;
            Point::new(line, logical_col(&self.buffer.line(line), desired as f32, self.tab_size))
        };
        self.set_caret(p, extend);
        self.desired_visual_col = Some(desired);
    }

    // ───────────── editing ─────────────

    fn indent_unit(&self) -> String {
        if self.insert_spaces { " ".repeat(self.tab_size) } else { "\t".into() }
    }

    /// Replaces the selection (or inserts at the caret).
    pub fn insert(&mut self, text: &str) {
        if self.read_only {
            return;
        }
        let (s, e) = self.selection();
        let end = self.buffer.replace(s, e, text);
        self.set_caret(end, false);
    }

    pub fn replace_range(&mut self, start: Point, end: Point, text: &str) {
        if self.read_only {
            return;
        }
        let end = self.buffer.replace(start, end, text);
        self.set_caret(end, false);
    }

    /// Typing with bracket/quote auto-closing, closer over-typing and `}` outdent.
    pub fn type_text(&mut self, text: &str) {
        if self.read_only || text.is_empty() {
            return;
        }
        let line: Vec<char> = self.buffer.line(self.caret.line).chars().collect();
        let col = self.caret.col.min(line.len());
        let next = line.get(col).copied();
        let prev = if col > 0 { line.get(col - 1).copied() } else { None };
        let mut chars = text.chars();
        if let (Some(c), None, true) = (chars.next(), chars.next(), self.auto_close) {
            let is_closer = matches!(c, ')' | ']' | '}' | '"' | '\'');
            if !self.has_selection() && is_closer && next == Some(c) {
                self.set_caret(Point::new(self.caret.line, col + 1), false);
                return;
            }
            if let Some(close) = closer_of(c) {
                let (s, e) = self.selection();
                if self.has_selection() && s.line == e.line {
                    let inner = self.buffer.slice(s, e);
                    self.buffer.replace(s, e, &format!("{c}{inner}{close}"));
                    let inner_len = inner.chars().count();
                    self.select(Point::new(s.line, s.col + 1), Point::new(s.line, s.col + 1 + inner_len));
                    return;
                }
                let quote = c == '"' || c == '\'';
                let next_ok = next.is_none_or(|n| n.is_whitespace() || matches!(n, ')' | ']' | '}' | ';' | ','));
                let prev_ok = !quote || prev.is_none_or(|p| !(p.is_alphanumeric() || p == c));
                if !self.has_selection() && next_ok && prev_ok {
                    self.buffer.replace(self.caret, self.caret, &format!("{c}{close}"));
                    self.set_caret(Point::new(self.caret.line, col + 1), false);
                    return;
                }
            }
            if c == '}' && !self.has_selection() && col > 0 && line[..col].iter().all(|ch| ch.is_whitespace()) {
                let indent: String = line[..col].iter().collect();
                let reduced = self.remove_indent_unit(&indent);
                let caret = self.caret;
                self.replace_range(Point::new(caret.line, 0), caret, &format!("{reduced}}}"));
                return;
            }
        }
        self.insert(text);
    }

    fn remove_indent_unit(&self, indent: &str) -> String {
        if let Some(stripped) = indent.strip_suffix('\t') {
            return stripped.to_owned();
        }
        let spaces = indent.chars().rev().take_while(|c| *c == ' ').count().min(self.tab_size);
        indent[..indent.len() - spaces].to_owned()
    }

    pub fn newline(&mut self) {
        if self.read_only {
            return;
        }
        let (s, e) = self.selection();
        let line: String = self.buffer.line(s.line).into_owned();
        let head: String = line.chars().take(s.col).collect();
        let indent: String = head.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        let before = head.trim_end();
        let after_line = self.buffer.line(e.line).into_owned();
        let after: String = after_line.chars().skip(e.col).collect::<String>().trim_start().to_owned();
        let opener = before.chars().last();

        if let Some(open @ ('{' | '(' | '[')) = opener {
            let inner = format!("{indent}{}", self.indent_unit());
            if after.starts_with(closer_of(open).unwrap_or(' ')) {
                self.buffer.replace(s, e, &format!("\n{inner}\n{indent}"));
                self.set_caret(Point::new(s.line + 1, inner.chars().count()), false);
            } else {
                self.insert(&format!("\n{inner}"));
            }
            return;
        }
        self.insert(&format!("\n{indent}"));
    }

    pub fn tab(&mut self) {
        if self.read_only {
            return;
        }
        let (s, e) = self.selection();
        if self.has_selection() && s.line != e.line {
            self.indent_lines(true);
            return;
        }
        if !self.insert_spaces {
            self.insert("\t");
            return;
        }
        let v = visual_col(&self.buffer.line(s.line), s.col, self.tab_size);
        self.insert(&" ".repeat(self.tab_size - v % self.tab_size));
    }

    fn selected_lines(&self) -> (usize, usize) {
        let (s, e) = self.selection();
        let last = if e.col == 0 && e.line > s.line { e.line - 1 } else { e.line };
        (s.line, last)
    }

    pub fn indent_lines(&mut self, indent: bool) {
        if self.read_only {
            return;
        }
        let (first, last) = self.selected_lines();
        let had_selection = self.has_selection();
        let unit = self.indent_unit();
        self.buffer.undo_stack().begin_group();
        for l in first..=last {
            let text = self.buffer.line(l).into_owned();
            if indent {
                if !text.is_empty() {
                    self.buffer.replace(Point::new(l, 0), Point::new(l, 0), &unit);
                }
            } else {
                let n = if text.starts_with('\t') { 1 } else { text.chars().take(self.tab_size).take_while(|c| *c == ' ').count() };
                if n > 0 {
                    self.buffer.replace(Point::new(l, 0), Point::new(l, n), "");
                }
            }
        }
        self.buffer.undo_stack().end_group();
        if had_selection {
            let end = self.buffer.line_len(last);
            self.select(Point::new(first, 0), Point::new(last, end));
        } else {
            self.clamp();
        }
    }

    pub fn toggle_comment(&mut self) {
        let Some(prefix) = self.line_comment else { return };
        if self.read_only {
            return;
        }
        let (first, last) = self.selected_lines();
        let lines: Vec<String> = (first..=last).map(|l| self.buffer.line(l).into_owned()).collect();
        let non_empty: Vec<&String> = lines.iter().filter(|l| !l.trim().is_empty()).collect();
        if non_empty.is_empty() {
            return;
        }
        let uncomment = non_empty.iter().all(|l| l.trim_start().starts_with(prefix));
        let min_indent = non_empty.iter().map(|l| l.chars().take_while(|c| c.is_whitespace()).count()).min().unwrap_or(0);
        let had_selection = self.has_selection();

        self.buffer.undo_stack().begin_group();
        for (i, text) in lines.iter().enumerate() {
            let l = first + i;
            if text.trim().is_empty() {
                continue;
            }
            if uncomment {
                let byte = text.find(prefix).unwrap_or(0);
                let col = text[..byte].chars().count();
                let mut len = prefix.chars().count();
                if text[byte + prefix.len()..].starts_with(' ') {
                    len += 1;
                }
                self.buffer.replace(Point::new(l, col), Point::new(l, col + len), "");
            } else {
                self.buffer.replace(Point::new(l, min_indent), Point::new(l, min_indent), &format!("{prefix} "));
            }
        }
        self.buffer.undo_stack().end_group();
        if had_selection {
            let end = self.buffer.line_len(last);
            self.select(Point::new(first, 0), Point::new(last, end));
        } else {
            self.clamp();
        }
    }

    pub fn move_lines(&mut self, down: bool) {
        if self.read_only {
            return;
        }
        let (first, last) = self.selected_lines();
        if (!down && first == 0) || (down && last + 1 >= self.buffer.line_count()) {
            return;
        }
        let (caret, anchor) = (self.caret, self.anchor);
        self.buffer.undo_stack().begin_group();
        if down {
            let below = self.buffer.line(last + 1).into_owned();
            let below_len = below.chars().count();
            self.buffer.replace(Point::new(last, self.buffer.line_len(last)), Point::new(last + 1, below_len), "");
            self.buffer.replace(Point::new(first, 0), Point::new(first, 0), &format!("{below}\n"));
        } else {
            let above = self.buffer.line(first - 1).into_owned();
            self.buffer.replace(Point::new(first - 1, 0), Point::new(first, 0), "");
            let end = Point::new(last - 1, self.buffer.line_len(last - 1));
            self.buffer.replace(end, end, &format!("\n{above}"));
        }
        self.buffer.undo_stack().end_group();
        let shift = |p: Point| Point::new(if down { p.line + 1 } else { p.line - 1 }, p.col);
        self.caret = shift(caret);
        self.anchor = anchor.map(shift);
    }

    pub fn duplicate_lines(&mut self) {
        if self.read_only {
            return;
        }
        let (first, last) = self.selected_lines();
        let block = self.buffer.slice(Point::new(first, 0), Point::new(last, self.buffer.line_len(last)));
        let end = Point::new(last, self.buffer.line_len(last));
        self.buffer.replace(end, end, &format!("\n{block}"));
        let count = last - first + 1;
        self.caret.line += count;
        self.anchor = self.anchor.map(|a| Point::new(a.line + count, a.col));
    }

    pub fn delete_lines(&mut self) {
        if self.read_only {
            return;
        }
        let (first, last) = self.selected_lines();
        let col = self.caret.col;
        if last + 1 < self.buffer.line_count() {
            self.buffer.replace(Point::new(first, 0), Point::new(last + 1, 0), "");
        } else if first > 0 {
            self.buffer.replace(Point::new(first - 1, self.buffer.line_len(first - 1)), Point::new(last, self.buffer.line_len(last)), "");
        } else {
            let end = self.buffer.end();
            self.buffer.replace(Point::default(), end, "");
        }
        self.set_caret(Point::new(first, col), false);
    }

    pub fn backspace(&mut self, word: bool) {
        if self.read_only {
            return;
        }
        if self.has_selection() {
            self.insert("");
            return;
        }
        let caret = self.caret;
        if caret == Point::default() {
            return;
        }
        let line: Vec<char> = self.buffer.line(caret.line).chars().collect();
        let col = caret.col;
        let from = if word {
            self.word_left(caret)
        } else if self.auto_close && col > 0 && col < line.len() && closer_of(line[col - 1]) == Some(line[col]) {
            self.replace_range(Point::new(caret.line, col - 1), Point::new(caret.line, col + 1), "");
            return;
        } else if self.insert_spaces && col > 0 && line[..col].iter().all(|c| *c == ' ') {
            Point::new(caret.line, col - ((col - 1) % self.tab_size + 1))
        } else {
            self.char_left(caret)
        };
        self.replace_range(from, caret, "");
    }

    pub fn delete_forward(&mut self, word: bool) {
        if self.read_only {
            return;
        }
        if self.has_selection() {
            self.insert("");
            return;
        }
        let caret = self.caret;
        let to = if word { self.word_right(caret) } else { self.char_right(caret) };
        if to != caret {
            self.replace_range(caret, to, "");
        }
    }

    /// Text for copy: the selection, or the whole current line (with newline) when nothing is selected.
    pub fn copy_text(&self) -> String {
        if self.has_selection() {
            self.selected_text()
        } else {
            format!("{}\n", self.buffer.line(self.caret.line))
        }
    }

    pub fn cut(&mut self) -> String {
        let text = self.copy_text();
        if self.has_selection() {
            self.insert("");
        } else {
            self.delete_lines();
        }
        text
    }

    pub fn undo(&mut self) {
        if let Some(p) = self.buffer.undo() {
            self.set_caret(p, false);
        }
    }

    pub fn redo(&mut self) {
        if let Some(p) = self.buffer.redo() {
            self.set_caret(p, false);
        }
    }

    /// Finds all occurrences of `query` (single line) as (start, end) points.
    pub fn find_all(&self, query: &str, match_case: bool, limit: usize) -> Vec<(Point, Point)> {
        let mut result = Vec::new();
        if query.is_empty() || query.contains('\n') {
            return result;
        }
        let needle = if match_case { query.to_owned() } else { query.to_lowercase() };
        let needle_len = query.chars().count();
        for l in 0..self.buffer.line_count() {
            let text = self.buffer.line(l);
            let hay = if match_case { text.to_string() } else { text.to_lowercase() };
            if hay.len() != text.len() {
                // Lowercasing changed byte lengths; fall back to exact matching for this line.
                for (byte, _) in text.match_indices(query) {
                    let col = text[..byte].chars().count();
                    result.push((Point::new(l, col), Point::new(l, col + needle_len)));
                }
                continue;
            }
            for (byte, _) in hay.match_indices(&needle) {
                let col = text[..byte].chars().count();
                result.push((Point::new(l, col), Point::new(l, col + needle_len)));
                if result.len() >= limit {
                    return result;
                }
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(text: &str) -> EditorState {
        let mut e = EditorState::new(Buffer::new(text));
        e.line_comment = Some("//");
        e
    }

    #[test]
    fn auto_close_and_overtype() {
        let mut e = editor("");
        e.type_text("(");
        assert_eq!(e.buffer.text(), "()");
        assert_eq!(e.caret(), Point::new(0, 1));
        e.type_text(")");
        assert_eq!(e.buffer.text(), "()");
        assert_eq!(e.caret(), Point::new(0, 2));
    }

    #[test]
    fn newline_between_braces_indents() {
        let mut e = editor("if (x) {}");
        e.set_caret(Point::new(0, 8), false);
        e.newline();
        assert_eq!(e.buffer.text(), "if (x) {\n    \n}");
        assert_eq!(e.caret(), Point::new(1, 4));
    }

    #[test]
    fn closing_brace_outdents() {
        let mut e = editor("{\n        ");
        e.set_caret(Point::new(1, 8), false);
        e.type_text("}");
        assert_eq!(e.buffer.line(1), "    }");
    }

    #[test]
    fn toggle_comment_round_trip() {
        let mut e = editor("a\n  b");
        e.select(Point::new(0, 0), Point::new(1, 3));
        e.toggle_comment();
        assert_eq!(e.buffer.text(), "// a\n//   b");
        e.toggle_comment();
        assert_eq!(e.buffer.text(), "a\n  b");
    }

    #[test]
    fn move_and_duplicate_lines() {
        let mut e = editor("one\ntwo\nthree");
        e.set_caret(Point::new(0, 1), false);
        e.move_lines(true);
        assert_eq!(e.buffer.text(), "two\none\nthree");
        assert_eq!(e.caret(), Point::new(1, 1));
        e.duplicate_lines();
        assert_eq!(e.buffer.text(), "two\none\none\nthree");
        e.buffer.undo();
        e.buffer.undo();
        assert_eq!(e.buffer.text(), "one\ntwo\nthree");
    }

    #[test]
    fn backspace_removes_indent_unit_and_pairs() {
        let mut e = editor("        x");
        e.set_caret(Point::new(0, 8), false);
        e.backspace(false);
        assert_eq!(e.buffer.text(), "    x");
        let mut e = editor("()");
        e.set_caret(Point::new(0, 1), false);
        e.backspace(false);
        assert_eq!(e.buffer.text(), "");
    }

    #[test]
    fn vertical_movement_keeps_column_across_tabs() {
        let mut e = editor("\tabc\nabcdefgh\nx");
        e.set_caret(Point::new(0, 2), false); // visual col 5
        e.move_vertical(1, false);
        assert_eq!(e.caret(), Point::new(1, 5));
        e.move_vertical(1, false);
        assert_eq!(e.caret(), Point::new(2, 1));
        e.move_vertical(-1, false);
        assert_eq!(e.caret(), Point::new(1, 5));
    }

    #[test]
    fn word_movement() {
        let e = editor("foo.bar  baz");
        assert_eq!(e.word_right(Point::new(0, 0)), Point::new(0, 3));
        assert_eq!(e.word_right(Point::new(0, 4)), Point::new(0, 9));
        assert_eq!(e.word_left(Point::new(0, 9)), Point::new(0, 4));
    }

    #[test]
    fn find_all_case_insensitive() {
        let e = editor("Foo foo\nFOO");
        assert_eq!(e.find_all("foo", false, 100).len(), 3);
        assert_eq!(e.find_all("foo", true, 100).len(), 1);
    }
}
