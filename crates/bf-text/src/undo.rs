use std::time::{Duration, Instant};

use crate::buffer::Point;

#[derive(Clone, Debug)]
pub(crate) struct Edit {
    pub start: Point,
    pub removed: String,
    pub inserted: String,
}

#[derive(Clone, Debug)]
pub(crate) struct Entry {
    pub id: u64,
    pub edits: Vec<Edit>,
    time: Instant,
}

/// Undo history with typing coalescing and explicit groups for multi-edit commands.
#[derive(Debug, Default)]
pub struct UndoStack {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    group_depth: usize,
    group_open: bool,
    next_id: u64,
}

const MERGE_WINDOW: Duration = Duration::from_millis(1500);
const MAX_ENTRIES: usize = 10_000;

impl UndoStack {
    pub fn state_id(&self) -> u64 {
        self.undo.last().map_or(0, |e| e.id)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// All edits until the matching [`end_group`](Self::end_group) undo as one step.
    pub fn begin_group(&mut self) {
        if self.group_depth == 0 {
            self.group_open = false;
        }
        self.group_depth += 1;
    }

    pub fn end_group(&mut self) {
        self.group_depth = self.group_depth.saturating_sub(1);
    }

    fn new_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub(crate) fn record(&mut self, start: Point, removed: &str, inserted: &str) {
        self.redo.clear();
        let edit = Edit { start, removed: removed.to_owned(), inserted: inserted.to_owned() };

        if self.group_depth > 0 {
            if !self.group_open {
                let id = self.new_id();
                self.undo.push(Entry { id, edits: Vec::new(), time: Instant::now() });
                self.group_open = true;
            }
            self.undo.last_mut().expect("group entry").edits.push(edit);
            return;
        }

        let id = self.new_id();
        if let Some(last) = self.undo.last_mut()
            && last.edits.len() == 1
            && last.time.elapsed() < MERGE_WINDOW
            && let Some(merged) = merge(&last.edits[0], &edit)
        {
            last.edits[0] = merged;
            last.time = Instant::now();
            last.id = id;
            return;
        }

        self.undo.push(Entry { id, edits: vec![edit], time: Instant::now() });
        if self.undo.len() > MAX_ENTRIES {
            self.undo.remove(0);
        }
    }

    pub(crate) fn pop_undo(&mut self) -> Option<Entry> {
        self.undo.pop()
    }

    pub(crate) fn pop_redo(&mut self) -> Option<Entry> {
        self.redo.pop()
    }

    pub(crate) fn push_undo(&mut self, entry: Entry) {
        self.undo.push(entry);
    }

    pub(crate) fn push_redo(&mut self, entry: Entry) {
        self.redo.push(entry);
    }
}

/// Coalesces typing, backspace and delete runs on a single line.
fn merge(prev: &Edit, next: &Edit) -> Option<Edit> {
    let no_newlines = [&prev.removed, &prev.inserted, &next.removed, &next.inserted].iter().all(|s| !s.contains('\n'));
    if !no_newlines {
        return None;
    }
    let len = |s: &str| s.chars().count();

    // Typing: contiguous single characters, broken when whitespace follows a word.
    if prev.removed.is_empty() && next.removed.is_empty() && len(&next.inserted) == 1
        && next.start == Point::new(prev.start.line, prev.start.col + len(&prev.inserted))
    {
        let typed = next.inserted.chars().next()?;
        let last = prev.inserted.chars().last();
        if typed.is_whitespace() && last.is_some_and(|c| !c.is_whitespace()) {
            return None;
        }
        return Some(Edit { start: prev.start, removed: String::new(), inserted: format!("{}{}", prev.inserted, next.inserted) });
    }

    // Backspace.
    if prev.inserted.is_empty() && next.inserted.is_empty() && len(&next.removed) == 1
        && prev.start.col > 0 && next.start == Point::new(prev.start.line, prev.start.col - 1)
    {
        return Some(Edit { start: next.start, removed: format!("{}{}", next.removed, prev.removed), inserted: String::new() });
    }

    // Delete key.
    if prev.inserted.is_empty() && next.inserted.is_empty() && len(&next.removed) == 1 && next.start == prev.start {
        return Some(Edit { start: prev.start, removed: format!("{}{}", prev.removed, next.removed), inserted: String::new() });
    }
    None
}
