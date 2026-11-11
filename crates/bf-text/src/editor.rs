use crate::buffer::{Buffer, Change, Point, order};

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

/// One caret with its optional selection anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub caret: Point,
    pub anchor: Option<Point>,
    desired_visual_col: Option<usize>,
}

impl Cursor {
    pub fn new(caret: Point, anchor: Option<Point>) -> Self {
        Self { caret, anchor, desired_visual_col: None }
    }

    pub fn has_selection(&self) -> bool {
        self.anchor.is_some_and(|a| a != self.caret)
    }

    /// Ordered (start, end); empty at the caret without a selection.
    pub fn selection(&self) -> (Point, Point) {
        match self.anchor {
            Some(a) if a != self.caret => order(a, self.caret),
            _ => (self.caret, self.caret),
        }
    }
}

/// Where `p` ends up after `change` (for positions at or after the change).
fn shift_point(p: Point, c: &Change) -> Point {
    if p < c.start {
        p
    } else if p < c.old_end {
        c.new_end
    } else if p.line == c.old_end.line {
        Point::new(c.new_end.line, c.new_end.col + (p.col - c.old_end.col))
    } else {
        Point::new(p.line - c.old_end.line + c.new_end.line, p.col)
    }
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
    /// Secondary cursors (multi-cursor editing); the primary one is `caret`/`anchor`.
    extra: Vec<Cursor>,
    /// Used by [`EditorState::toggle_comment`] when there is no line comment (e.g. Razor's `@*` `*@`).
    pub block_comment: Option<(&'static str, &'static str)>,
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
            block_comment: None,
            extra: Vec::new(),
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

    /// Moves the caret (collapsing any extra cursors).
    pub fn set_caret(&mut self, p: Point, extend: bool) {
        self.extra.clear();
        self.place(p, extend);
    }

    /// Selects `anchor..caret` (collapsing any extra cursors).
    pub fn select(&mut self, anchor: Point, caret: Point) {
        self.extra.clear();
        self.select_range(anchor, caret);
    }

    fn place(&mut self, p: Point, extend: bool) {
        let p = self.buffer.clamp(p);
        if extend {
            self.anchor.get_or_insert(self.caret);
        } else {
            self.anchor = None;
        }
        self.caret = p;
        self.desired_visual_col = None;
    }

    fn select_range(&mut self, anchor: Point, caret: Point) {
        self.anchor = Some(self.buffer.clamp(anchor));
        self.caret = self.buffer.clamp(caret);
        self.desired_visual_col = None;
    }

    pub fn select_all(&mut self) {
        self.extra.clear();
        let end = self.buffer.end();
        self.select_range(Point::default(), end);
    }

    pub fn clear_selection(&mut self) {
        self.anchor = None;
        for c in &mut self.extra {
            c.anchor = None;
        }
    }

    /// Re-clamps caret/anchor after external buffer changes.
    pub fn clamp(&mut self) {
        self.caret = self.buffer.clamp(self.caret);
        self.anchor = self.anchor.map(|a| self.buffer.clamp(a));
        for c in &mut self.extra {
            c.caret = self.buffer.clamp(c.caret);
            c.anchor = c.anchor.map(|a| self.buffer.clamp(a));
        }
        if !self.extra.is_empty() {
            self.merge_cursors();
        }
    }

    // ───────────── multiple cursors ─────────────

    pub fn has_multiple_cursors(&self) -> bool {
        !self.extra.is_empty()
    }

    pub fn cursor_count(&self) -> usize {
        self.extra.len() + 1
    }

    fn primary(&self) -> Cursor {
        Cursor { caret: self.caret, anchor: self.anchor, desired_visual_col: self.desired_visual_col }
    }

    fn set_primary(&mut self, c: Cursor) {
        self.caret = c.caret;
        self.anchor = c.anchor;
        self.desired_visual_col = c.desired_visual_col;
    }

    /// Every cursor (the primary one included) in document order, with whether it's the primary.
    pub fn cursors(&self) -> Vec<(Cursor, bool)> {
        let mut all: Vec<(Cursor, bool)> = self.extra.iter().map(|c| (*c, false)).collect();
        all.push((self.primary(), true));
        all.sort_by_key(|(c, _)| c.selection().0);
        all
    }

    /// Sets one cursor per selection `(anchor, caret)`; `primary` is the index of the primary one.
    pub fn set_selections(&mut self, selections: &[(Point, Point)], primary: usize) {
        self.extra.clear();
        for (i, &(anchor, caret)) in selections.iter().enumerate() {
            let c = if anchor == caret { Cursor::new(caret, None) } else { Cursor::new(caret, Some(anchor)) };
            if i == primary {
                self.set_primary(c);
            } else {
                self.extra.push(c);
            }
        }
        self.clamp();
        self.merge_cursors();
    }

    /// Drops the extra cursors, keeping the primary one.
    pub fn collapse_cursors(&mut self) {
        self.extra.clear();
    }

    /// Alt+Click: adds a cursor at `p`, or removes the one already there.
    pub fn toggle_cursor(&mut self, p: Point) {
        let p = self.buffer.clamp(p);
        if let Some(i) = self.extra.iter().position(|c| c.caret == p) {
            self.extra.remove(i);
            return;
        }
        if self.caret == p {
            if let Some(next) = self.extra.pop() {
                self.set_primary(next);
            }
            return;
        }
        self.extra.push(self.primary());
        self.set_primary(Cursor::new(p, None));
        self.merge_cursors();
    }

    /// Ctrl+Alt+Up/Down: a new cursor on the line above the topmost (below the bottommost) cursor, in the
    /// primary cursor's visual column. Returns the new caret.
    pub fn add_cursor_vertical(&mut self, down: bool) -> Option<Point> {
        let all = self.cursors();
        let edge = if down { all.iter().map(|(c, _)| c.caret.line).max()? } else { all.iter().map(|(c, _)| c.caret.line).min()? };
        let target = if down { edge + 1 } else { edge.checked_sub(1)? };
        if target >= self.buffer.line_count() {
            return None;
        }
        let desired = self.desired_visual_col.unwrap_or_else(|| visual_col(&self.buffer.line(self.caret.line), self.caret.col, self.tab_size));
        let p = Point::new(target, logical_col(&self.buffer.line(target), desired as f32, self.tab_size));
        self.extra.push(Cursor { anchor: None, ..self.primary() });
        self.set_primary(Cursor { caret: p, anchor: None, desired_visual_col: Some(desired) });
        self.merge_cursors();
        Some(p)
    }

    /// Ctrl+D: selects the word at the caret, or adds a cursor selecting the next occurrence of the
    /// primary selection (wrapping around). Returns the newly selected range.
    pub fn add_next_match(&mut self) -> Option<(Point, Point)> {
        if !self.has_selection() {
            let (s, e) = self.word_at(self.caret);
            if s == e || self.buffer.slice(s, e).trim().is_empty() {
                return None;
            }
            self.select_range(s, e);
            return Some((s, e));
        }
        let query = self.selected_text();
        let taken: Vec<(Point, Point)> = self.cursors().iter().map(|(c, _)| c.selection()).collect();
        let matches = self.find_all(&query, true, 100_000);
        let after = self.selection().1;
        let next = matches.iter().filter(|m| !taken.contains(m)).find(|m| m.0 >= after).or_else(|| matches.iter().find(|m| !taken.contains(m)))?;
        let (s, e) = *next;
        self.extra.push(self.primary());
        self.set_primary(Cursor::new(e, Some(s)));
        Some((s, e))
    }

    /// Ctrl+Shift+L: a cursor on every occurrence of the selection (or of the word at the caret).
    pub fn select_all_matches(&mut self) -> usize {
        if !self.has_selection() && self.add_next_match().is_none() {
            return 0;
        }
        let query = self.selected_text();
        let primary = self.selection();
        let matches = self.find_all(&query, true, 100_000);
        self.extra = matches.iter().filter(|m| **m != primary).map(|&(s, e)| Cursor::new(e, Some(s))).collect();
        self.cursor_count()
    }

    /// Runs a single-cursor command once per cursor, from the bottom of the document up, so each
    /// edit only shifts cursors already done (which are moved along). One undo step for all.
    fn for_each_cursor(&mut self, mut f: impl FnMut(&mut Self)) {
        if self.extra.is_empty() {
            f(self);
            return;
        }
        let mut todo: Vec<(Cursor, bool)> = self.extra.drain(..).map(|c| (c, false)).collect();
        todo.push((self.primary(), true));
        todo.sort_by_key(|(c, _)| std::cmp::Reverse(c.selection().0));
        let mut done: Vec<(Cursor, bool)> = Vec::with_capacity(todo.len());
        self.buffer.undo_stack().begin_group();
        for (cursor, is_primary) in todo {
            self.set_primary(cursor);
            let before = self.buffer.changes_len();
            f(self);
            for change in self.buffer.changes_since(before) {
                for (c, _) in &mut done {
                    c.caret = shift_point(c.caret, change);
                    c.anchor = c.anchor.map(|a| shift_point(a, change));
                }
            }
            done.push((self.primary(), is_primary));
        }
        self.buffer.undo_stack().end_group();
        for (c, is_primary) in done {
            if is_primary {
                self.set_primary(c);
            } else {
                self.extra.push(c);
            }
        }
        self.merge_cursors();
    }

    // ───────────── line commands with several cursors ─────────────

    /// The lines each cursor covers (a selection ending at column 0 leaves that line out).
    fn cursor_lines(c: &Cursor) -> (usize, usize) {
        let (s, e) = c.selection();
        (s.line, if e.col == 0 && e.line > s.line { e.line - 1 } else { e.line })
    }

    /// The line blocks of all cursors, top to bottom, merged when they overlap (and when they touch, with
    /// `touching`, so moved blocks don't run into each other).
    fn cursor_line_blocks(&self, touching: bool) -> Vec<(usize, usize)> {
        let mut ranges: Vec<(usize, usize)> = self.cursors().iter().map(|(c, _)| Self::cursor_lines(c)).collect();
        ranges.sort_unstable();
        let mut blocks: Vec<(usize, usize)> = Vec::new();
        for (s, e) in ranges {
            match blocks.last_mut() {
                Some(last) if s <= last.1 || (touching && s == last.1 + 1) => last.1 = last.1.max(e),
                _ => blocks.push((s, e)),
            }
        }
        blocks
    }

    /// Puts the cursors back after `edit`, each moved over the changes it made.
    fn keep_cursors_over(&mut self, edit: impl FnOnce(&mut Self)) {
        let cursors = self.cursors();
        let before = self.buffer.changes_len();
        self.buffer.undo_stack().begin_group();
        edit(self);
        self.buffer.undo_stack().end_group();
        let changes: Vec<Change> = self.buffer.changes_since(before).to_vec();
        self.extra.clear();
        for (mut c, is_primary) in cursors {
            for change in &changes {
                c.caret = shift_point(c.caret, change);
                c.anchor = c.anchor.map(|a| shift_point(a, change));
            }
            if is_primary {
                self.set_primary(c);
            } else {
                self.extra.push(c);
            }
        }
        self.clamp();
        self.merge_cursors();
    }

    /// Comments (or, when every non-empty line is commented, uncomments) the lines of all cursors; each
    /// block gets the comment at its own indentation.
    fn toggle_comment_multi(&mut self, prefix: &str, suffix: &str) {
        let blocks = self.cursor_line_blocks(false);
        let lines: Vec<(usize, String)> = blocks.iter().flat_map(|&(s, e)| s..=e).map(|l| (l, self.buffer.line(l).into_owned())).collect();
        let non_empty: Vec<&String> = lines.iter().map(|(_, t)| t).filter(|t| !t.trim().is_empty()).collect();
        if non_empty.is_empty() {
            return;
        }
        let uncomment = non_empty.iter().all(|l| l.trim_start().starts_with(prefix) && l.trim_end().ends_with(suffix));
        let (prefix, suffix) = (prefix.to_owned(), suffix.to_owned());
        self.keep_cursors_over(|s| {
            for &(first, last) in &blocks {
                let block: Vec<String> = (first..=last).map(|l| s.buffer.line(l).into_owned()).collect();
                let min_indent = block.iter().filter(|l| !l.trim().is_empty()).map(|l| l.chars().take_while(|c| c.is_whitespace()).count()).min().unwrap_or(0);
                for (i, text) in block.iter().enumerate() {
                    let l = first + i;
                    if text.trim().is_empty() {
                        continue;
                    }
                    if uncomment {
                        if !suffix.is_empty() {
                            let end = text.trim_end().len();
                            let mut byte = end - suffix.len();
                            if text[..byte].ends_with(' ') {
                                byte -= 1;
                            }
                            let (from, to) = (text[..byte].chars().count(), text[..end].chars().count());
                            s.buffer.replace(Point::new(l, from), Point::new(l, to), "");
                        }
                        let byte = text.find(prefix.as_str()).unwrap_or(0);
                        let col = text[..byte].chars().count();
                        let mut len = prefix.chars().count();
                        if text[byte + prefix.len()..].starts_with(' ') {
                            len += 1;
                        }
                        s.buffer.replace(Point::new(l, col), Point::new(l, col + len), "");
                    } else {
                        if !suffix.is_empty() {
                            let end = text.trim_end().chars().count();
                            s.buffer.replace(Point::new(l, end), Point::new(l, end), &format!(" {suffix}"));
                        }
                        s.buffer.replace(Point::new(l, min_indent), Point::new(l, min_indent), &format!("{prefix} "));
                    }
                }
            }
        });
    }

    /// Moves the lines of every cursor up or down by one; nothing moves when a block is at the edge.
    fn move_lines_multi(&mut self, down: bool) {
        let blocks = self.cursor_line_blocks(true);
        let (Some(&(top, _)), Some(&(_, bottom))) = (blocks.first(), blocks.last()) else { return };
        if (!down && top == 0) || (down && bottom + 1 >= self.buffer.line_count()) {
            return;
        }
        let cursors = self.cursors();
        self.buffer.undo_stack().begin_group();
        // Touching blocks are merged, so each one swaps with a line no other block has.
        let order: Vec<(usize, usize)> = if down { blocks.iter().rev().copied().collect() } else { blocks.clone() };
        for (first, last) in order {
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
        }
        self.buffer.undo_stack().end_group();
        let shift = |p: Point| Point::new(if down { p.line + 1 } else { p.line - 1 }, p.col);
        self.restore_cursors(cursors, |c| Cursor { caret: shift(c.caret), anchor: c.anchor.map(shift), ..c });
    }

    /// Duplicates the lines of every cursor below themselves; the cursors move to the copies.
    fn duplicate_lines_multi(&mut self) {
        let blocks = self.cursor_line_blocks(false);
        let cursors = self.cursors();
        self.buffer.undo_stack().begin_group();
        for &(first, last) in blocks.iter().rev() {
            let block = self.buffer.slice(Point::new(first, 0), Point::new(last, self.buffer.line_len(last)));
            let end = Point::new(last, self.buffer.line_len(last));
            self.buffer.replace(end, end, &format!("\n{block}"));
        }
        self.buffer.undo_stack().end_group();
        // A cursor moves down by the size of its own block and of every block above it.
        let added = |line: usize| blocks.iter().filter(|b| b.0 <= line).map(|b| b.1 - b.0 + 1).sum::<usize>();
        self.restore_cursors(cursors, |c| {
            let n = added(c.selection().0.line);
            Cursor { caret: Point::new(c.caret.line + n, c.caret.col), anchor: c.anchor.map(|a| Point::new(a.line + n, a.col)), ..c }
        });
    }

    /// Deletes the lines of every cursor; each cursor ends up where its lines were.
    fn delete_lines_multi(&mut self) {
        let blocks = self.cursor_line_blocks(true);
        let cursors = self.cursors();
        self.buffer.undo_stack().begin_group();
        for &(first, last) in blocks.iter().rev() {
            if last + 1 < self.buffer.line_count() {
                self.buffer.replace(Point::new(first, 0), Point::new(last + 1, 0), "");
            } else if first > 0 {
                self.buffer.replace(Point::new(first - 1, self.buffer.line_len(first - 1)), Point::new(last, self.buffer.line_len(last)), "");
            } else {
                let end = self.buffer.end();
                self.buffer.replace(Point::default(), end, "");
            }
        }
        self.buffer.undo_stack().end_group();
        let removed_above = |line: usize| blocks.iter().filter(|b| b.1 < line).map(|b| b.1 - b.0 + 1).sum::<usize>();
        self.restore_cursors(cursors, |c| {
            let first = blocks.iter().find(|b| b.0 <= c.caret.line && c.caret.line <= b.1).map_or(c.caret.line, |b| b.0);
            Cursor::new(Point::new(first - removed_above(first), c.caret.col), None)
        });
    }

    /// Sets the cursors to `cursors` moved by `f`, clamped and merged.
    fn restore_cursors(&mut self, cursors: Vec<(Cursor, bool)>, f: impl Fn(Cursor) -> Cursor) {
        self.extra.clear();
        for (c, is_primary) in cursors {
            let c = f(c);
            if is_primary {
                self.set_primary(c);
            } else {
                self.extra.push(c);
            }
        }
        self.clamp();
        self.merge_cursors();
    }

    /// Replaces the word before every cursor (when it is `prefix`, the word the primary cursor's completion
    /// was asked for) with `text`: a completion accepted with several cursors.
    pub fn complete_at_cursors(&mut self, prefix: &str, text: &str) {
        let len = prefix.chars().count();
        self.for_each_cursor(|s| {
            let caret = s.caret;
            if caret.col < len {
                return;
            }
            let start = Point::new(caret.line, caret.col - len);
            if s.buffer.slice(start, caret) == prefix {
                s.replace_one(start, caret, text);
            }
        });
    }

    /// Joins cursors that sit on the same spot or whose selections overlap.
    fn merge_cursors(&mut self) {
        let all = self.cursors();
        let mut merged: Vec<(Cursor, bool)> = Vec::with_capacity(all.len());
        for (c, is_primary) in all {
            if let Some((last, last_primary)) = merged.last_mut() {
                let (ls, le) = last.selection();
                let (s, e) = c.selection();
                if s == ls || s < le || (c.caret == last.caret) {
                    let (start, end) = (ls.min(s), le.max(e));
                    *last = if start == end { Cursor::new(end, None) } else { Cursor::new(end, Some(start)) };
                    *last_primary |= is_primary;
                    continue;
                }
            }
            merged.push((c, is_primary));
        }
        self.extra.clear();
        for (c, is_primary) in merged {
            if is_primary {
                self.set_primary(c);
            } else {
                self.extra.push(c);
            }
        }
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
        self.for_each_cursor(|s| s.move_left_one(word, extend));
    }

    fn move_left_one(&mut self, word: bool, extend: bool) {
        if !extend && !word && self.has_selection() {
            let (s, _) = self.selection();
            self.place(s, false);
        } else {
            let p = if word { self.word_left(self.caret) } else { self.char_left(self.caret) };
            self.place(p, extend);
        }
    }

    pub fn move_right(&mut self, word: bool, extend: bool) {
        self.for_each_cursor(|s| s.move_right_one(word, extend));
    }

    fn move_right_one(&mut self, word: bool, extend: bool) {
        if !extend && !word && self.has_selection() {
            let (_, e) = self.selection();
            self.place(e, false);
        } else {
            let p = if word { self.word_right(self.caret) } else { self.char_right(self.caret) };
            self.place(p, extend);
        }
    }

    /// Vertical movement keeping the desired visual column (like every code editor).
    pub fn move_vertical(&mut self, delta: isize, extend: bool) {
        self.for_each_cursor(|s| s.move_vertical_one(delta, extend));
    }

    /// Home (smart: first non-blank, then column 0) at every cursor.
    pub fn move_home(&mut self, extend: bool) {
        self.for_each_cursor(|s| s.place(s.smart_home(s.caret), extend));
    }

    pub fn move_end(&mut self, extend: bool) {
        self.for_each_cursor(|s| s.place(s.line_end(s.caret), extend));
    }

    fn move_vertical_one(&mut self, delta: isize, extend: bool) {
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
        self.place(p, extend);
        self.desired_visual_col = Some(desired);
    }

    // ───────────── editing ─────────────

    /// One level of indentation: `tab_size` spaces or a tab.
    pub fn indent_unit(&self) -> String {
        if self.insert_spaces { " ".repeat(self.tab_size) } else { "\t".into() }
    }

    /// Replaces the selection (or inserts at the caret), at every cursor.
    pub fn insert(&mut self, text: &str) {
        self.for_each_cursor(|s| s.insert_one(text));
    }

    /// Paste: with one line of clipboard text per cursor, each cursor gets its own line.
    pub fn paste(&mut self, text: &str) {
        let text = text.replace("\r\n", "\n");
        let lines: Vec<&str> = text.strip_suffix('\n').unwrap_or(&text).split('\n').collect();
        if self.has_multiple_cursors() && lines.len() == self.cursor_count() {
            // Cursors run bottom-up: hand out the lines from the end.
            let mut rest = lines.into_iter().rev();
            self.for_each_cursor(|s| s.insert_one(rest.next().unwrap_or_default()));
        } else {
            self.insert(&text);
        }
    }

    fn insert_one(&mut self, text: &str) {
        if self.read_only {
            return;
        }
        let (s, e) = self.selection();
        let end = self.buffer.replace(s, e, text);
        self.place(end, false);
    }

    /// Replaces `start..end` and puts the caret after it (collapsing any extra cursors).
    pub fn replace_range(&mut self, start: Point, end: Point, text: &str) {
        self.extra.clear();
        self.replace_one(start, end, text);
    }

    fn replace_one(&mut self, start: Point, end: Point, text: &str) {
        if self.read_only {
            return;
        }
        let end = self.buffer.replace(start, end, text);
        self.place(end, false);
    }

    /// Typing with bracket/quote auto-closing, closer over-typing and `}` outdent.
    pub fn type_text(&mut self, text: &str) {
        self.for_each_cursor(|s| s.type_text_one(text));
    }

    fn type_text_one(&mut self, text: &str) {
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
                self.place(Point::new(self.caret.line, col + 1), false);
                return;
            }
            if let Some(close) = closer_of(c) {
                let (s, e) = self.selection();
                if self.has_selection() && s.line == e.line {
                    let inner = self.buffer.slice(s, e);
                    self.buffer.replace(s, e, &format!("{c}{inner}{close}"));
                    let inner_len = inner.chars().count();
                    self.select_range(Point::new(s.line, s.col + 1), Point::new(s.line, s.col + 1 + inner_len));
                    return;
                }
                let quote = c == '"' || c == '\'';
                let next_ok = next.is_none_or(|n| n.is_whitespace() || matches!(n, ')' | ']' | '}' | ';' | ','));
                let prev_ok = !quote || prev.is_none_or(|p| !(p.is_alphanumeric() || p == c));
                if !self.has_selection() && next_ok && prev_ok {
                    self.buffer.replace(self.caret, self.caret, &format!("{c}{close}"));
                    self.place(Point::new(self.caret.line, col + 1), false);
                    return;
                }
            }
            if c == '}' && !self.has_selection() && col > 0 && line[..col].iter().all(|ch| ch.is_whitespace()) {
                let indent: String = line[..col].iter().collect();
                let reduced = self.remove_indent_unit(&indent);
                let caret = self.caret;
                self.replace_one(Point::new(caret.line, 0), caret, &format!("{reduced}}}"));
                return;
            }
        }
        self.insert_one(text);
    }

    fn remove_indent_unit(&self, indent: &str) -> String {
        if let Some(stripped) = indent.strip_suffix('\t') {
            return stripped.to_owned();
        }
        let spaces = indent.chars().rev().take_while(|c| *c == ' ').count().min(self.tab_size);
        indent[..indent.len() - spaces].to_owned()
    }

    pub fn newline(&mut self) {
        self.for_each_cursor(Self::newline_one);
    }

    fn newline_one(&mut self) {
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
                self.place(Point::new(s.line + 1, inner.chars().count()), false);
            } else {
                self.insert_one(&format!("\n{inner}"));
            }
            return;
        }
        self.insert_one(&format!("\n{indent}"));
    }

    pub fn tab(&mut self) {
        self.for_each_cursor(Self::tab_one);
    }

    fn tab_one(&mut self) {
        if self.read_only {
            return;
        }
        let (s, e) = self.selection();
        if self.has_selection() && s.line != e.line {
            self.indent_lines(true);
            return;
        }
        if !self.insert_spaces {
            self.insert_one("\t");
            return;
        }
        let v = visual_col(&self.buffer.line(s.line), s.col, self.tab_size);
        self.insert_one(&" ".repeat(self.tab_size - v % self.tab_size));
    }

    fn selected_lines(&self) -> (usize, usize) {
        let (s, e) = self.selection();
        let last = if e.col == 0 && e.line > s.line { e.line - 1 } else { e.line };
        (s.line, last)
    }

    pub fn indent_lines(&mut self, indent: bool) {
        self.extra.clear();
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
            self.select_range(Point::new(first, 0), Point::new(last, end));
        } else {
            self.clamp();
        }
    }

    pub fn toggle_comment(&mut self) {
        let (prefix, suffix) = match (self.line_comment, self.block_comment) {
            (Some(prefix), _) => (prefix, ""),
            (None, Some((open, close))) => (open, close),
            (None, None) => return,
        };
        if self.read_only {
            return;
        }
        if self.has_multiple_cursors() {
            self.toggle_comment_multi(prefix, suffix);
            return;
        }
        let (first, last) = self.selected_lines();
        let lines: Vec<String> = (first..=last).map(|l| self.buffer.line(l).into_owned()).collect();
        let non_empty: Vec<&String> = lines.iter().filter(|l| !l.trim().is_empty()).collect();
        if non_empty.is_empty() {
            return;
        }
        let uncomment = non_empty.iter().all(|l| l.trim_start().starts_with(prefix) && l.trim_end().ends_with(suffix));
        let min_indent = non_empty.iter().map(|l| l.chars().take_while(|c| c.is_whitespace()).count()).min().unwrap_or(0);
        let had_selection = self.has_selection();

        self.buffer.undo_stack().begin_group();
        for (i, text) in lines.iter().enumerate() {
            let l = first + i;
            if text.trim().is_empty() {
                continue;
            }
            if uncomment {
                if !suffix.is_empty() {
                    // Remove the suffix first so the prefix columns stay valid.
                    let end = text.trim_end().len();
                    let mut byte = end - suffix.len();
                    if text[..byte].ends_with(' ') {
                        byte -= 1;
                    }
                    let (from, to) = (text[..byte].chars().count(), text[..end].chars().count());
                    self.buffer.replace(Point::new(l, from), Point::new(l, to), "");
                }
                let byte = text.find(prefix).unwrap_or(0);
                let col = text[..byte].chars().count();
                let mut len = prefix.chars().count();
                if text[byte + prefix.len()..].starts_with(' ') {
                    len += 1;
                }
                self.buffer.replace(Point::new(l, col), Point::new(l, col + len), "");
            } else {
                if !suffix.is_empty() {
                    let end = text.trim_end().chars().count();
                    self.buffer.replace(Point::new(l, end), Point::new(l, end), &format!(" {suffix}"));
                }
                self.buffer.replace(Point::new(l, min_indent), Point::new(l, min_indent), &format!("{prefix} "));
            }
        }
        self.buffer.undo_stack().end_group();
        if had_selection {
            let end = self.buffer.line_len(last);
            self.select_range(Point::new(first, 0), Point::new(last, end));
        } else {
            self.clamp();
        }
    }

    pub fn move_lines(&mut self, down: bool) {
        if self.read_only {
            return;
        }
        if self.has_multiple_cursors() {
            self.move_lines_multi(down);
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
        if self.has_multiple_cursors() {
            self.duplicate_lines_multi();
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
        if self.has_multiple_cursors() {
            self.delete_lines_multi();
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
        self.place(Point::new(first, col), false);
    }

    pub fn backspace(&mut self, word: bool) {
        self.for_each_cursor(|s| s.backspace_one(word));
    }

    fn backspace_one(&mut self, word: bool) {
        if self.read_only {
            return;
        }
        if self.has_selection() {
            self.insert_one("");
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
            self.replace_one(Point::new(caret.line, col - 1), Point::new(caret.line, col + 1), "");
            return;
        } else if self.insert_spaces && col > 0 && line[..col].iter().all(|c| *c == ' ') {
            Point::new(caret.line, col - ((col - 1) % self.tab_size + 1))
        } else {
            self.char_left(caret)
        };
        self.replace_one(from, caret, "");
    }

    pub fn delete_forward(&mut self, word: bool) {
        self.for_each_cursor(|s| s.delete_forward_one(word));
    }

    fn delete_forward_one(&mut self, word: bool) {
        if self.read_only {
            return;
        }
        if self.has_selection() {
            self.insert_one("");
            return;
        }
        let caret = self.caret;
        let to = if word { self.word_right(caret) } else { self.char_right(caret) };
        if to != caret {
            self.replace_one(caret, to, "");
        }
    }

    /// Text for copy: the selection, or the whole current line (with newline) when nothing is selected.
    pub fn copy_text(&self) -> String {
        if self.has_multiple_cursors() {
            let all = self.cursors();
            let any_selection = all.iter().any(|(c, _)| c.has_selection());
            let parts: Vec<String> = all
                .iter()
                .map(|(c, _)| if any_selection { let (s, e) = c.selection(); self.buffer.slice(s, e) } else { self.buffer.line(c.caret.line).into_owned() })
                .collect();
            return if any_selection { parts.join("\n") } else { format!("{}\n", parts.join("\n")) };
        }
        if self.has_selection() {
            self.selected_text()
        } else {
            format!("{}\n", self.buffer.line(self.caret.line))
        }
    }

    pub fn cut(&mut self) -> String {
        let text = self.copy_text();
        if self.has_multiple_cursors() && self.cursors().iter().any(|(c, _)| c.has_selection()) {
            self.insert("");
        } else if self.has_selection() {
            self.insert_one("");
        } else {
            self.delete_lines();
        }
        text
    }

    pub fn undo(&mut self) {
        self.extra.clear();
        if let Some(p) = self.buffer.undo() {
            self.place(p, false);
        }
    }

    pub fn redo(&mut self) {
        self.extra.clear();
        if let Some(p) = self.buffer.redo() {
            self.place(p, false);
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
    fn toggle_block_comment_round_trip() {
        let mut e = editor("<p>a</p>\n  @x");
        e.line_comment = None;
        e.block_comment = Some(("@*", "*@"));
        e.select(Point::new(0, 0), Point::new(1, 4));
        e.toggle_comment();
        assert_eq!(e.buffer.text(), "@* <p>a</p> *@\n@*   @x *@");
        e.toggle_comment();
        assert_eq!(e.buffer.text(), "<p>a</p>\n  @x");
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

    #[test]
    fn multi_cursor_typing_and_backspace() {
        let mut e = editor("ab\ncd\nef");
        e.set_caret(Point::new(0, 1), false);
        e.toggle_cursor(Point::new(1, 1));
        e.toggle_cursor(Point::new(2, 1));
        assert_eq!(e.cursor_count(), 3);
        e.type_text("X");
        assert_eq!(e.buffer.text(), "aXb\ncXd\neXf");
        e.backspace(false);
        assert_eq!(e.buffer.text(), "ab\ncd\nef");
        // one undo step for the whole multi-cursor edit
        e.type_text("Y");
        e.undo();
        assert_eq!(e.buffer.text(), "ab\ncd\nef");
    }

    #[test]
    fn multi_cursor_line_commands() {
        let mut e = editor("a\nb\n    c\nd\ne");
        e.line_comment = Some("//");
        e.set_caret(Point::new(0, 1), false);
        e.toggle_cursor(Point::new(2, 5));
        e.toggle_cursor(Point::new(4, 0));
        e.toggle_comment();
        assert_eq!(e.buffer.text(), "// a\nb\n    // c\nd\n// e");
        let carets: Vec<Point> = e.cursors().iter().map(|(c, _)| c.caret).collect();
        assert_eq!(carets, vec![Point::new(0, 4), Point::new(2, 8), Point::new(4, 3)]);
        e.toggle_comment();
        assert_eq!(e.buffer.text(), "a\nb\n    c\nd\ne");
        e.undo();
        assert_eq!(e.buffer.text(), "// a\nb\n    // c\nd\n// e", "one undo step for all cursors");
        e.undo();

        // Move: lines 0 and 2 go down; line 4 is the last, so with it nothing moves.
        e.set_caret(Point::new(0, 0), false);
        e.toggle_cursor(Point::new(2, 0));
        e.move_lines(true);
        assert_eq!(e.buffer.text(), "b\na\nd\n    c\ne");
        assert_eq!(e.cursors().iter().map(|(c, _)| c.caret.line).collect::<Vec<_>>(), vec![1, 3]);
        e.move_lines(false);
        assert_eq!(e.buffer.text(), "a\nb\n    c\nd\ne");

        e.duplicate_lines();
        assert_eq!(e.buffer.text(), "a\na\nb\n    c\n    c\nd\ne");
        assert_eq!(e.cursors().iter().map(|(c, _)| c.caret.line).collect::<Vec<_>>(), vec![1, 4]);
        e.delete_lines();
        assert_eq!(e.buffer.text(), "a\nb\n    c\nd\ne");
        assert_eq!(e.cursors().iter().map(|(c, _)| c.caret.line).collect::<Vec<_>>(), vec![1, 3]);
    }

    #[test]
    fn completion_at_every_cursor() {
        let mut e = editor("Con\nCon\nCo");
        e.set_caret(Point::new(0, 3), false);
        e.toggle_cursor(Point::new(1, 3));
        e.toggle_cursor(Point::new(2, 2));
        e.complete_at_cursors("Con", "Console");
        // The last cursor typed something else, so it stays as it was.
        assert_eq!(e.buffer.text(), "Console\nConsole\nCo");
    }

    #[test]
    fn multi_cursor_on_one_line_shifts_later_cursors() {
        let mut e = editor("a a a");
        e.set_caret(Point::new(0, 0), false);
        e.toggle_cursor(Point::new(0, 2));
        e.toggle_cursor(Point::new(0, 4));
        e.type_text("(");
        assert_eq!(e.buffer.text(), "(a (a (a");
        e.newline();
        assert_eq!(e.buffer.line_count(), 4);
        let carets: Vec<Point> = e.cursors().iter().map(|(c, _)| c.caret).collect();
        assert_eq!(carets, vec![Point::new(1, 4), Point::new(2, 4), Point::new(3, 4)]);
    }

    #[test]
    fn add_next_match_and_select_all_matches() {
        let mut e = editor("foo bar foo\nfoo");
        e.set_caret(Point::new(0, 1), false);
        assert_eq!(e.add_next_match(), Some((Point::new(0, 0), Point::new(0, 3))));
        assert_eq!(e.add_next_match(), Some((Point::new(0, 8), Point::new(0, 11))));
        assert_eq!(e.add_next_match(), Some((Point::new(1, 0), Point::new(1, 3))));
        assert_eq!(e.add_next_match(), None);
        e.type_text("x");
        assert_eq!(e.buffer.text(), "x bar x\nx");
        let mut e = editor("a.b a.b");
        e.select(Point::new(0, 0), Point::new(0, 3));
        assert_eq!(e.select_all_matches(), 2);
    }

    #[test]
    fn vertical_cursors_copy_and_paste_per_line() {
        let mut e = editor("one\ntwo\nthree");
        e.set_caret(Point::new(0, 0), false);
        e.add_cursor_vertical(true);
        e.add_cursor_vertical(true);
        assert_eq!(e.add_cursor_vertical(true), None);
        e.move_end(true);
        assert_eq!(e.copy_text(), "one\ntwo\nthree");
        e.move_home(false);
        e.paste("1\n2\n3");
        assert_eq!(e.buffer.text(), "1one\n2two\n3three");
        e.set_caret(Point::new(0, 0), false);
        assert!(!e.has_multiple_cursors());
    }

    #[test]
    fn overlapping_cursors_merge() {
        let mut e = editor("abc");
        e.set_caret(Point::new(0, 1), false);
        e.toggle_cursor(Point::new(0, 2));
        e.move_left(false, false);
        e.move_left(false, false);
        assert_eq!(e.cursor_count(), 1);
    }
}
