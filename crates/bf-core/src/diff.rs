//! The diff editor's model: two texts compared line by line, laid out side by side (aligned with filler
//! lines) or inline (removed lines above the added ones), with the changed words inside changed lines.

use similar::{Algorithm, DiffOp, TextDiff};

/// What a row of the diff shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Unchanged,
    Removed,
    Added,
    /// An empty row that keeps the two sides aligned.
    Filler,
}

/// One pane of the diff: its text, a gutter number per line, and the highlights.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pane {
    pub text: String,
    /// The line number in the file (1-based); `None` on filler rows.
    pub numbers: Vec<Option<usize>>,
    pub kinds: Vec<RowKind>,
    /// Changed words: `(row, start char, end char)`.
    pub words: Vec<(usize, usize, usize)>,
}

impl Pane {
    fn push(&mut self, line: &str, number: Option<usize>, kind: RowKind) -> usize {
        if !self.kinds.is_empty() {
            self.text.push('\n');
        }
        self.text.push_str(line);
        self.numbers.push(number);
        self.kinds.push(kind);
        self.kinds.len() - 1
    }
}

/// A comparison of two texts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Diff {
    /// Side by side: the old text on the left, the new on the right; row `i` of one is row `i` of the other.
    pub left: Pane,
    pub right: Pane,
    /// The first row of each change, side by side.
    pub hunks: Vec<usize>,
    /// Inline: unchanged lines once, removed lines above the lines that replaced them.
    pub inline: Pane,
    /// The first row of each change, inline.
    pub inline_hunks: Vec<usize>,
    /// For each change, the new file's line (0-based) it is at, to open the file there.
    pub hunk_lines: Vec<usize>,
    pub added: usize,
    pub removed: usize,
}

impl Diff {
    pub fn is_unchanged(&self) -> bool {
        self.hunks.is_empty()
    }
}

/// Compares `old` with `new` line by line.
pub fn compare(old: &str, new: &str) -> Diff {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let diff = TextDiff::configure().algorithm(Algorithm::Myers).timeout(std::time::Duration::from_secs(2)).diff_lines(old, new);
    let mut out = Diff::default();
    // Consecutive deletes and inserts form one change, shown as a replacement.
    let mut pending: Option<(usize, usize, usize, usize)> = None;
    for op in diff.ops() {
        match *op {
            DiffOp::Equal { old_index, new_index, len } => {
                if let Some(change) = pending.take() {
                    push_change(&mut out, &old_lines, &new_lines, change);
                }
                for i in 0..len {
                    let (o, n) = (old_index + i, new_index + i);
                    let line = old_lines.get(o).copied().unwrap_or_default();
                    out.left.push(line, Some(o + 1), RowKind::Unchanged);
                    out.right.push(new_lines.get(n).copied().unwrap_or_default(), Some(n + 1), RowKind::Unchanged);
                    out.inline.push(new_lines.get(n).copied().unwrap_or_default(), Some(n + 1), RowKind::Unchanged);
                }
            }
            DiffOp::Delete { old_index, old_len, new_index } => extend(&mut pending, (old_index, old_len, new_index, 0)),
            DiffOp::Insert { old_index, new_index, new_len } => extend(&mut pending, (old_index, 0, new_index, new_len)),
            DiffOp::Replace { old_index, old_len, new_index, new_len } => extend(&mut pending, (old_index, old_len, new_index, new_len)),
        }
    }
    if let Some(change) = pending.take() {
        push_change(&mut out, &old_lines, &new_lines, change);
    }
    out
}

/// Grows the pending change `(old start, old len, new start, new len)` by an adjacent one.
fn extend(pending: &mut Option<(usize, usize, usize, usize)>, next: (usize, usize, usize, usize)) {
    *pending = Some(match *pending {
        Some((o, ol, n, nl)) => (o, ol + next.1, n, nl + next.3),
        None => next,
    });
}

fn push_change(out: &mut Diff, old_lines: &[&str], new_lines: &[&str], (o, ol, n, nl): (usize, usize, usize, usize)) {
    let old: Vec<&str> = (o..o + ol).map(|i| old_lines.get(i).copied().unwrap_or_default()).collect();
    let new: Vec<&str> = (n..n + nl).map(|i| new_lines.get(i).copied().unwrap_or_default()).collect();
    out.removed += ol;
    out.added += nl;
    out.hunks.push(out.left.kinds.len());
    out.inline_hunks.push(out.inline.kinds.len());
    out.hunk_lines.push(n.min(new_lines.len().saturating_sub(1)));
    // Lines replaced one for one get their changed words marked.
    let words: Vec<Option<WordChanges>> =
        (0..ol.max(nl)).map(|i| (i < ol && i < nl).then(|| changed_words(old[i], new[i])).flatten()).collect();

    for i in 0..ol.max(nl) {
        let row = match old.get(i) {
            Some(line) => out.left.push(line, Some(o + i + 1), RowKind::Removed),
            None => out.left.push("", None, RowKind::Filler),
        };
        if let Some(Some((removed, _))) = words.get(i) {
            out.left.words.extend(removed.iter().map(|&(s, e)| (row, s, e)));
        }
        let row = match new.get(i) {
            Some(line) => out.right.push(line, Some(n + i + 1), RowKind::Added),
            None => out.right.push("", None, RowKind::Filler),
        };
        if let Some(Some((_, added))) = words.get(i) {
            out.right.words.extend(added.iter().map(|&(s, e)| (row, s, e)));
        }
    }
    for (i, line) in old.iter().enumerate() {
        let row = out.inline.push(line, Some(o + i + 1), RowKind::Removed);
        if let Some(Some((removed, _))) = words.get(i) {
            out.inline.words.extend(removed.iter().map(|&(s, e)| (row, s, e)));
        }
    }
    for (i, line) in new.iter().enumerate() {
        let row = out.inline.push(line, Some(n + i + 1), RowKind::Added);
        if let Some(Some((_, added))) = words.get(i) {
            out.inline.words.extend(added.iter().map(|&(s, e)| (row, s, e)));
        }
    }
}

/// Changed character ranges `(start, end)` of a replaced line: (in the old line, in the new line).
type WordChanges = (Vec<(usize, usize)>, Vec<(usize, usize)>);

/// Splits a line into words, runs of whitespace and single symbols, as `(start char, text)`.
fn tokens(line: &str) -> Vec<(usize, String)> {
    let class = |c: char| if c.is_alphanumeric() || c == '_' { 0 } else if c.is_whitespace() { 1 } else { 2 };
    let mut out: Vec<(usize, String)> = Vec::new();
    let mut last = None;
    for (i, c) in line.chars().enumerate() {
        let k = class(c);
        match out.last_mut() {
            Some((_, text)) if last == Some(k) && k != 2 => text.push(c),
            _ => out.push((i, c.to_string())),
        }
        last = Some(k);
    }
    out
}

/// The changed character ranges of a line replaced by another, word by word: `(removed, added)`. `None`
/// when most of the line changed (then the line background says enough).
fn changed_words(old: &str, new: &str) -> Option<WordChanges> {
    if old.trim().is_empty() || new.trim().is_empty() {
        return None;
    }
    let (a, b) = (tokens(old), tokens(new));
    let a_text: Vec<&str> = a.iter().map(|t| t.1.as_str()).collect();
    let b_text: Vec<&str> = b.iter().map(|t| t.1.as_str()).collect();
    let ops = similar::capture_diff_slices(Algorithm::Myers, &a_text, &b_text);
    let span = |toks: &[(usize, String)], from: usize, len: usize| -> Option<(usize, usize)> {
        let first = toks.get(from)?;
        let last = toks.get(from + len - 1)?;
        Some((first.0, last.0 + last.1.chars().count()))
    };
    let (mut removed, mut added) = (Vec::new(), Vec::new());
    let mut same = 0;
    for op in &ops {
        match *op {
            DiffOp::Equal { old_index, len, .. } => same += (old_index..old_index + len).map(|i| a[i].1.chars().count()).sum::<usize>(),
            DiffOp::Delete { old_index, old_len, .. } => removed.extend(span(&a, old_index, old_len)),
            DiffOp::Insert { new_index, new_len, .. } => added.extend(span(&b, new_index, new_len)),
            DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                removed.extend(span(&a, old_index, old_len));
                added.extend(span(&b, new_index, new_len));
            }
        }
    }
    // Mostly rewritten: marking nearly every word helps nobody.
    let longest = old.chars().count().max(new.chars().count()).max(1);
    (same * 10 >= longest * 4).then_some((removed, added))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_by_side_rows_line_up() {
        let d = compare("a\nb\nc\nd\n", "a\nB\nc\nnew\nd\n");
        assert_eq!(d.left.kinds.len(), d.right.kinds.len());
        assert_eq!(d.left.text, "a\nb\nc\n\nd");
        assert_eq!(d.right.text, "a\nB\nc\nnew\nd");
        assert_eq!(d.left.numbers, [Some(1), Some(2), Some(3), None, Some(4)]);
        assert_eq!(d.right.numbers, [Some(1), Some(2), Some(3), Some(4), Some(5)]);
        assert_eq!(d.left.kinds[3], RowKind::Filler);
        assert_eq!(d.right.kinds[3], RowKind::Added);
        assert_eq!(d.hunks, [1, 3]);
        assert_eq!(d.hunk_lines, [1, 3]);
        assert_eq!((d.added, d.removed), (2, 1));
    }

    #[test]
    fn inline_puts_removed_lines_above_added_ones() {
        let d = compare("a\nold\nz\n", "a\nnew\nz\n");
        assert_eq!(d.inline.text, "a\nold\nnew\nz");
        assert_eq!(d.inline.kinds, [RowKind::Unchanged, RowKind::Removed, RowKind::Added, RowKind::Unchanged]);
        assert_eq!(d.inline.numbers, [Some(1), Some(2), Some(2), Some(3)]);
        assert_eq!(d.inline_hunks, [1]);
    }

    #[test]
    fn marks_the_changed_words() {
        let d = compare("var total = price * count;\n", "var total = price * amount;\n");
        assert_eq!(d.left.words, [(0, 20, 25)]);
        assert_eq!(d.right.words, [(0, 20, 26)]);
        assert_eq!(d.inline.words, [(0, 20, 25), (1, 20, 26)]);
    }

    #[test]
    fn rewritten_lines_get_no_word_marks() {
        let d = compare("int a = 1;\n", "Console.WriteLine(message);\n");
        assert!(d.left.words.is_empty() && d.right.words.is_empty());
    }

    #[test]
    fn whole_file_added_or_removed() {
        let d = compare("", "one\ntwo\n");
        assert_eq!(d.left.numbers, [None, None]);
        assert_eq!(d.right.kinds, [RowKind::Added, RowKind::Added]);
        assert_eq!(d.hunks, [0]);
        let d = compare("one\n", "");
        assert_eq!(d.right.kinds, [RowKind::Filler]);
        assert_eq!(d.hunk_lines, [0]);
        assert!(compare("same\n", "same\n").is_unchanged());
    }
}
