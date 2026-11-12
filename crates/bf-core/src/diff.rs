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
    /// For each change, the lines it replaces in the old text and the lines that replace them in the new.
    pub changes: Vec<LineChange>,
    pub added: usize,
    pub removed: usize,
}

/// One change as line ranges (0-based start, count): `old` lines are replaced by `new` lines. An insertion
/// has no old lines (its `old.0` is where they go), a deletion no new ones.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LineChange {
    pub old: (usize, usize),
    pub new: (usize, usize),
}

/// The lines of `text` with their line endings.
fn raw_lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// `base` with lines `at.0 .. at.0 + at.1` replaced by lines `from.0 .. from.0 + from.1` of `source`.
/// Line endings come along with their lines; a line moved to the end of the text without one, or put
/// before others without one, gets the text's line ending fixed up.
fn splice(base: &str, at: (usize, usize), source: &str, from: (usize, usize)) -> String {
    let (b, s) = (raw_lines(base), raw_lines(source));
    let newline = if base.contains("\r\n") || source.contains("\r\n") { "\r\n" } else { "\n" };
    let start = at.0.min(b.len());
    let end = (at.0 + at.1).min(b.len());
    let mut out: Vec<String> = Vec::with_capacity(b.len() + from.1);
    out.extend(b[..start].iter().map(|l| (*l).to_owned()));
    out.extend(s.iter().skip(from.0).take(from.1).map(|l| (*l).to_owned()));
    out.extend(b[end..].iter().map(|l| (*l).to_owned()));
    // Every line but the last needs its line ending.
    let n = out.len();
    for line in out.iter_mut().take(n.saturating_sub(1)) {
        if !line.ends_with('\n') {
            line.push_str(newline);
        }
    }
    out.concat()
}

/// `old` with one change applied: what staging just that change puts in the index.
pub fn apply_change(old: &str, new: &str, change: &LineChange) -> String {
    splice(old, change.old, new, change.new)
}

/// `new` with one change taken back: what reverting (or unstaging) just that change leaves.
pub fn revert_change(old: &str, new: &str, change: &LineChange) -> String {
    splice(new, change.new, old, change.old)
}

/// Rows to hide when unchanged regions are collapsed: for each run of unchanged rows longer than
/// `2 * context + 1`, the fold `(first, last)` whose first row stays visible (as the "⋯" line) and that
/// leaves `context` rows next to each change.
pub fn unchanged_folds(kinds: &[RowKind], context: usize) -> Vec<(usize, usize)> {
    let mut folds = Vec::new();
    let mut i = 0;
    while i < kinds.len() {
        if kinds[i] != RowKind::Unchanged {
            i += 1;
            continue;
        }
        let start = i;
        while i < kinds.len() && kinds[i] == RowKind::Unchanged {
            i += 1;
        }
        // Keep context after the change above and before the change below (none at the text's edges).
        let from = if start == 0 { 0 } else { start + context };
        let to = if i == kinds.len() { i - 1 } else { (i - 1).saturating_sub(context) };
        if to > from + 1 {
            folds.push((from, to));
        }
    }
    folds
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
    out.changes.push(LineChange { old: (o, ol), new: (n, nl) });
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
    fn stages_and_reverts_one_change() {
        let old = "a\nb\nc\nd\n";
        let new = "a\nB\nc\nnew\nd\n";
        let d = compare(old, new);
        assert_eq!(d.changes, [LineChange { old: (1, 1), new: (1, 1) }, LineChange { old: (3, 0), new: (3, 1) }]);
        // Staging the first change only: the index gets `B` but not `new`.
        assert_eq!(apply_change(old, new, &d.changes[0]), "a\nB\nc\nd\n");
        assert_eq!(apply_change(old, new, &d.changes[1]), "a\nb\nc\nnew\nd\n");
        // Reverting the second change only: the working file loses `new` but keeps `B`.
        assert_eq!(revert_change(old, new, &d.changes[1]), "a\nB\nc\nd\n");
        assert_eq!(revert_change(old, new, &d.changes[0]), "a\nb\nc\nnew\nd\n");
        // Applying every change, last first, gives the new text.
        let all = d.changes.iter().rev().fold(old.to_owned(), |text, c| apply_change(&text, new, c));
        assert_eq!(all, new);
    }

    #[test]
    fn line_endings_and_missing_final_newlines_survive() {
        let old = "a\r\nb\r\n";
        let new = "a\r\nb\r\nc";
        let d = compare(&old.replace("\r\n", "\n"), &new.replace("\r\n", "\n"));
        assert_eq!(apply_change(old, new, &d.changes[0]), "a\r\nb\r\nc");
        assert_eq!(revert_change(old, new, &d.changes[0]), old);
        // A line without a line ending that ends up before another gets one.
        let (old, new) = ("x\ny", "x\ny\nz\n");
        let d = compare(old, new);
        assert_eq!(apply_change(old, new, &d.changes[0]), new);
    }

    #[test]
    fn collapses_long_unchanged_runs() {
        use RowKind::*;
        let mut kinds = vec![Unchanged; 10];
        kinds.push(Added);
        kinds.extend(vec![Unchanged; 12]);
        kinds.push(Removed);
        kinds.extend(vec![Unchanged; 4]);
        // Rows 0..=9 before the first change: 3 rows of context stay (7, 8, 9); 0 stays as the fold's line.
        // Rows 11..=22 between the changes: context 11-13 and 20-22; 14 stays as the fold's line.
        // The 4 rows at the end are too few to fold.
        assert_eq!(unchanged_folds(&kinds, 3), [(0, 6), (14, 19)]);
        assert!(unchanged_folds(&[Unchanged, Added, Unchanged], 3).is_empty());
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
