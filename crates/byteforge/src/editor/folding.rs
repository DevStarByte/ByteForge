//! Code folding: which line ranges can fold (brace blocks, `#region`s, indentation for brace-less
//! languages), which are folded, and the mapping between buffer lines and screen rows that hiding lines
//! needs. A folded region keeps its first line visible and hides the rest, closing brace included.

use bf_text::{Change, Point};
use gpui::Context;

use super::EditorView;

/// How regions are found.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Style {
    /// `{ … }` blocks (strings and comments skipped) and `#region` … `#endregion` (C#, JSON, Razor).
    Braces,
    /// Lines indented deeper than the line before them (XML, YAML, Dockerfile, plain text).
    Indent,
}

#[derive(Default)]
pub(crate) struct Folds {
    /// Foldable regions `(first, last)` line, sorted by first line, for `regions_version`.
    regions: Vec<(usize, usize)>,
    regions_version: Option<u64>,
    /// Folded regions, sorted by first line.
    folded: Vec<(usize, usize)>,
    /// Hidden lines as merged inclusive ranges (each fold hides `first + 1 ..= last`).
    hidden: Vec<(usize, usize)>,
}

impl Folds {
    pub fn is_empty(&self) -> bool {
        self.folded.is_empty()
    }

    pub fn is_hidden(&self, line: usize) -> bool {
        self.hidden_range(line).is_some()
    }

    /// The hidden range containing `line`.
    pub fn hidden_range(&self, line: usize) -> Option<(usize, usize)> {
        let i = self.hidden.partition_point(|h| h.1 < line);
        self.hidden.get(i).copied().filter(|h| h.0 <= line)
    }

    /// The screen row of `line` (a hidden line shares the row of the visible line above it).
    pub fn row_of(&self, line: usize) -> usize {
        let mut hidden_before = 0;
        for &(s, e) in &self.hidden {
            if s > line {
                break;
            }
            if line <= e {
                // Hidden ranges start after a fold's first line, so `s >= 1`.
                return s - 1 - hidden_before;
            }
            hidden_before += e + 1 - s;
        }
        line - hidden_before
    }

    /// The buffer line shown on screen row `row`.
    pub fn line_of(&self, row: usize) -> usize {
        let mut line = row;
        for &(s, e) in &self.hidden {
            if s > line {
                break;
            }
            line += e + 1 - s;
        }
        line
    }

    /// Screen rows for a buffer of `line_count` lines.
    pub fn row_count(&self, line_count: usize) -> usize {
        line_count - self.hidden.iter().map(|(s, e)| e + 1 - s).sum::<usize>()
    }

    /// Hidden lines as merged inclusive ranges.
    pub fn hidden_ranges(&self) -> &[(usize, usize)] {
        &self.hidden
    }

    pub fn folded(&self) -> &[(usize, usize)] {
        &self.folded
    }

    fn rebuild_hidden(&mut self) {
        self.folded.sort_unstable();
        self.folded.dedup();
        let mut hidden: Vec<(usize, usize)> = Vec::new();
        for &(s, e) in &self.folded {
            let (s, e) = (s + 1, e);
            if s > e {
                continue;
            }
            match hidden.last_mut() {
                Some(last) if s <= last.1 + 1 => last.1 = last.1.max(e),
                _ => hidden.push((s, e)),
            }
        }
        self.hidden = hidden;
    }

    /// Regions for the current text (recomputed when the buffer version changes).
    pub fn regions(&mut self, version: u64, lines: impl FnOnce() -> Vec<String>, style: Style) -> &[(usize, usize)] {
        if self.regions_version != Some(version) {
            let lines = lines();
            self.regions = regions(&lines, style);
            self.regions_version = Some(version);
        }
        &self.regions
    }

    /// Cached regions without recomputing (may be from an older version).
    pub fn cached_regions(&self) -> &[(usize, usize)] {
        &self.regions
    }

    pub fn fold(&mut self, region: (usize, usize)) {
        self.folded.push(region);
        self.rebuild_hidden();
    }

    /// Unfolds every fold that hides `line` or starts on it.
    pub fn unfold_at(&mut self, line: usize) -> bool {
        let before = self.folded.len();
        self.folded.retain(|&(s, e)| !(s == line || (s < line && line <= e)));
        self.rebuild_hidden();
        self.folded.len() != before
    }

    pub fn set_folded(&mut self, folded: Vec<(usize, usize)>) {
        self.folded = folded;
        self.rebuild_hidden();
    }

    /// Keeps folds on their lines while editing. An edit inside a folded body, or one that adds or removes
    /// lines on its first line (Enter at its end), unfolds it.
    pub fn shift(&mut self, c: &Change) {
        if self.folded.is_empty() {
            return;
        }
        let delta = c.new_end.line as isize - c.old_end.line as isize;
        let moved = |line: usize| if line > c.old_end.line { (line as isize + delta).max(0) as usize } else { line };
        self.folded = self
            .folded
            .iter()
            .filter(|&&(s, e)| !(c.start.line <= e && c.old_end.line >= s && (c.old_end.line > s || delta != 0)))
            .map(|&(s, e)| (moved(s), moved(e)))
            .collect();
        self.rebuild_hidden();
    }
}

/// Foldable regions of `lines`, sorted by first line, at most one per first line (the largest).
pub(crate) fn regions(lines: &[String], style: Style) -> Vec<(usize, usize)> {
    let mut out = match style {
        Style::Braces => {
            let mut r = brace_regions(lines);
            r.extend(comment_and_using_regions(lines));
            r
        }
        Style::Indent => indent_regions(lines),
    };
    out.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    out.dedup_by_key(|r| r.0);
    out
}

/// Runs of `//` (and `///` doc) comment lines, multi-line `/* … */` comments, and the block of `using`
/// directives (blank lines between them included): each foldable when it spans two lines or more.
fn comment_and_using_regions(lines: &[String]) -> Vec<(usize, usize)> {
    let is_using = |t: &str| {
        let t = t.strip_prefix("global ").unwrap_or(t);
        t.starts_with("using ") && t.ends_with(';') && !t.contains('(') && !t.starts_with("using var ")
    };
    let mut out = Vec::new();
    let mut comment_run: Option<(usize, usize)> = None;
    let mut usings: Option<(usize, usize)> = None;
    let mut ix = 0;
    while ix < lines.len() {
        let t = lines[ix].trim();
        // A `/* … */` comment that starts a line and ends on a later one.
        if t.starts_with("/*")
            && !t.contains("*/")
            && let Some(end) = (ix + 1..lines.len()).find(|&j| lines[j].contains("*/"))
        {
            out.push((ix, end));
            ix = end + 1;
            continue;
        }
        if t.starts_with("//") {
            comment_run = Some(comment_run.map_or((ix, ix), |(s, _)| (s, ix)));
        } else if let Some((s, e)) = comment_run.take()
            && e > s
        {
            out.push((s, e));
        }
        if is_using(t) {
            usings = Some(usings.map_or((ix, ix), |(s, _)| (s, ix)));
        } else if !t.is_empty()
            && let Some((s, e)) = usings.take()
            && e > s
        {
            out.push((s, e));
        }
        ix += 1;
    }
    for (s, e) in [comment_run, usings].into_iter().flatten() {
        if e > s {
            out.push((s, e));
        }
    }
    out
}

fn brace_regions(lines: &[String]) -> Vec<(usize, usize)> {
    #[derive(PartialEq)]
    enum State {
        Code,
        Block,
        Verbatim,
        Raw,
    }
    let mut state = State::Code;
    let mut braces: Vec<usize> = Vec::new();
    let mut regions_stack: Vec<usize> = Vec::new();
    let mut out = Vec::new();
    for (ix, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if state == State::Code {
            if trimmed.starts_with("#region") {
                regions_stack.push(ix);
                continue;
            }
            if trimmed.starts_with("#endregion") {
                if let Some(start) = regions_stack.pop()
                    && ix > start
                {
                    out.push((start, ix));
                }
                continue;
            }
        }
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            let next = chars.get(i + 1).copied();
            match state {
                State::Block => {
                    if c == '*' && next == Some('/') {
                        state = State::Code;
                        i += 1;
                    }
                }
                State::Verbatim => {
                    if c == '"' {
                        if next == Some('"') {
                            i += 1;
                        } else {
                            state = State::Code;
                        }
                    }
                }
                State::Raw => {
                    if c == '"' && next == Some('"') && chars.get(i + 2) == Some(&'"') {
                        state = State::Code;
                        i += 2;
                    }
                }
                State::Code => match c {
                    '/' if next == Some('/') => break,
                    '/' if next == Some('*') => {
                        state = State::Block;
                        i += 1;
                    }
                    '"' if next == Some('"') && chars.get(i + 2) == Some(&'"') => {
                        state = State::Raw;
                        i += 2;
                    }
                    '@' if next == Some('"') => {
                        state = State::Verbatim;
                        i += 1;
                    }
                    '$' if next == Some('@') && chars.get(i + 2) == Some(&'"') => {
                        state = State::Verbatim;
                        i += 2;
                    }
                    '"' | '\'' => {
                        // A one-line string or char literal.
                        let quote = c;
                        i += 1;
                        while i < chars.len() && chars[i] != quote {
                            if chars[i] == '\\' {
                                i += 1;
                            }
                            i += 1;
                        }
                    }
                    '{' => braces.push(ix),
                    '}' => {
                        if let Some(open) = braces.pop()
                            && ix > open
                        {
                            // Allman style: the block belongs to the declaration line above a lone `{`.
                            let start = if open > 0 && lines[open].trim() == "{" && !lines[open - 1].trim().is_empty() { open - 1 } else { open };
                            out.push((start, ix));
                        }
                    }
                    _ => {}
                },
            }
            i += 1;
        }
    }
    out
}

fn indent_regions(lines: &[String]) -> Vec<(usize, usize)> {
    let indent = |line: &str| -> Option<usize> {
        if line.trim().is_empty() {
            None
        } else {
            Some(line.chars().take_while(|c| c.is_whitespace()).map(|c| if c == '\t' { 4 } else { 1 }).sum())
        }
    };
    let levels: Vec<Option<usize>> = lines.iter().map(|l| indent(l)).collect();
    let mut out = Vec::new();
    for (i, level) in levels.iter().enumerate() {
        let Some(level) = level else { continue };
        let mut end = i;
        for (j, l) in levels.iter().enumerate().skip(i + 1) {
            match l {
                Some(l) if *l <= *level => break,
                Some(_) => end = j,
                None => {}
            }
        }
        // XML: keep the closing tag inside the fold when it lines up with the opening one.
        if end > i
            && let Some(close) = lines.get(end + 1).filter(|l| l.trim_start().starts_with("</") && levels[end + 1] == Some(*level))
        {
            let _ = close;
            end += 1;
        }
        if end > i {
            out.push((i, end));
        }
    }
    out
}

impl EditorView {
    fn fold_style(&self) -> Style {
        match self.language {
            Some(bf_syntax::Language::CSharp | bf_syntax::Language::Json | bf_syntax::Language::Razor) => Style::Braces,
            _ => Style::Indent,
        }
    }

    /// Foldable regions of the current text.
    pub(crate) fn fold_regions(&mut self) -> Vec<(usize, usize)> {
        let version = self.state.buffer.version();
        let style = self.fold_style();
        let buffer = &self.state.buffer;
        self.folds
            .regions(version, || (0..buffer.line_count()).map(|l| buffer.line(l).into_owned()).collect(), style)
            .to_vec()
    }

    /// Folds or unfolds the region starting on `line` (gutter click).
    pub(crate) fn toggle_fold(&mut self, line: usize, cx: &mut Context<Self>) {
        if self.folds.unfold_at(line) {
            self.after_fold_change(cx);
            return;
        }
        if let Some(&region) = self.fold_regions().iter().find(|r| r.0 == line) {
            self.folds.fold(region);
            self.after_fold_change(cx);
        }
    }

    /// Ctrl+Shift+[: folds the innermost unfolded region around the caret.
    pub(crate) fn fold_at_caret(&mut self, cx: &mut Context<Self>) {
        let line = self.state.caret().line;
        let folded = self.folds.folded().to_vec();
        let region = self
            .fold_regions()
            .into_iter()
            .filter(|r| r.0 <= line && line <= r.1 && !folded.contains(r))
            .min_by_key(|r| r.1 - r.0);
        if let Some(region) = region {
            self.folds.fold(region);
            self.state.set_caret(Point::new(region.0, self.state.buffer.line_len(region.0)), false);
            self.after_fold_change(cx);
        }
    }

    /// Ctrl+Shift+]: unfolds the fold on the caret's line.
    pub(crate) fn unfold_at_caret(&mut self, cx: &mut Context<Self>) {
        if self.folds.unfold_at(self.state.caret().line) {
            self.after_fold_change(cx);
        }
    }

    pub(crate) fn fold_all(&mut self, cx: &mut Context<Self>) {
        let regions = self.fold_regions();
        // Fold the outermost regions only, so unfolding one shows its members (themselves foldable).
        let mut top: Vec<(usize, usize)> = Vec::new();
        for r in regions {
            if !top.iter().any(|t| t.0 <= r.0 && r.1 <= t.1) {
                top.push(r);
            }
        }
        // Namespaces and classes would hide everything: fold one level deeper inside a sole top region.
        if top.len() == 1 {
            let outer = top[0];
            let inner: Vec<(usize, usize)> = self.fold_regions().into_iter().filter(|r| *r != outer && outer.0 <= r.0 && r.1 <= outer.1).collect();
            let mut level: Vec<(usize, usize)> = Vec::new();
            for r in inner {
                if !level.iter().any(|t| t.0 <= r.0 && r.1 <= t.1) {
                    level.push(r);
                }
            }
            if !level.is_empty() {
                top = level;
            }
        }
        self.folds.set_folded(top);
        let caret = self.state.caret();
        if self.folds.is_hidden(caret.line) {
            let line = (0..caret.line).rev().find(|l| !self.folds.is_hidden(*l)).unwrap_or(0);
            self.state.set_caret(Point::new(line, 0), false);
        }
        self.after_fold_change(cx);
    }

    pub(crate) fn unfold_all(&mut self, cx: &mut Context<Self>) {
        if !self.folds.is_empty() {
            self.folds.set_folded(Vec::new());
            self.after_fold_change(cx);
        }
    }

    fn after_fold_change(&mut self, cx: &mut Context<Self>) {
        self.scroll = self.clamp_scroll(self.scroll);
        self.scroll_target = self.clamp_scroll(self.scroll_target);
        self.after_move(cx);
    }

    /// The caret landed on a hidden line: arrow keys step over the fold, anything else (go to line, find,
    /// go to definition) unfolds it.
    pub(crate) fn caret_into_fold(&mut self, previous_line: usize) {
        let caret = self.state.caret();
        let Some((s, e)) = self.folds.hidden_range(caret.line) else { return };
        let extend = self.state.has_selection();
        let anchor = self.state.anchor();
        let target = if previous_line + 1 == s && caret.line > previous_line {
            // Moving down from the fold's first line: past the fold (if there's a line after it).
            (e + 1 < self.state.buffer.line_count()).then(|| Point::new(e + 1, caret.col))
        } else if previous_line == e + 1 && caret.line < previous_line {
            // Moving up from below the fold: onto its first line.
            Some(Point::new(s - 1, caret.col))
        } else {
            None
        };
        match target {
            Some(p) => match (extend, anchor) {
                (true, Some(a)) => self.state.select(a, p),
                _ => self.state.set_caret(p, false),
            },
            None => {
                while self.folds.is_hidden(caret.line) {
                    let line = caret.line;
                    let Some(&(fs, _)) = self.folds.folded().iter().find(|&&(fs, fe)| fs < line && line <= fe) else { break };
                    self.folds.unfold_at(fs);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_owned).collect()
    }

    #[test]
    fn brace_regions_skip_strings_and_comments() {
        let text = lines(
            "namespace Demo\n{\n    public class A\n    {\n        // { not a brace\n        string s = \"}{\";\n        /* {\n        } */\n        void M() { }\n        void N()\n        {\n            var x = @\"{\n}\";\n        }\n    }\n    #region Helpers\n    #endregion\n}",
        );
        let r = regions(&text, Style::Braces);
        // namespace (0..17), class (2..14), the /* */ comment (6..7), N (9..13), region (15..16).
        assert_eq!(r, vec![(0, 17), (2, 14), (6, 7), (9, 13), (15, 16)]);
    }

    #[test]
    fn using_blocks_and_comment_runs_fold() {
        let text = lines(
            "using System;\nusing System.Linq;\n\nusing Foo;\n\nnamespace Demo;\n\n/// <summary>\n/// A thing.\n/// </summary>\npublic class A\n{\n    // one line\n    void M()\n    {\n        using var s = Open();\n        using var t = Open();\n    }\n}",
        );
        let r = regions(&text, Style::Braces);
        assert!(r.contains(&(0, 3)), "usings with blank lines between: {r:?}");
        assert!(r.contains(&(7, 9)), "doc comment: {r:?}");
        assert!(!r.iter().any(|&(s, _)| s == 12), "a single comment line doesn't fold: {r:?}");
        assert!(!r.iter().any(|&(s, _)| s == 15), "using statements aren't directives: {r:?}");
    }

    #[test]
    fn indent_regions_for_markup() {
        let text = lines("<Project>\n  <PropertyGroup>\n    <A>1</A>\n  </PropertyGroup>\n</Project>");
        assert_eq!(regions(&text, Style::Indent), vec![(0, 4), (1, 3)]);
        let yaml = lines("services:\n  api:\n    image: x\n\n  db:\n    image: y\nvolumes: {}");
        assert_eq!(regions(&yaml, Style::Indent), vec![(0, 5), (1, 2), (4, 5)]);
    }

    #[test]
    fn rows_and_lines() {
        let mut f = Folds::default();
        f.fold((2, 5)); // hides 3..=5
        f.fold((8, 9)); // hides 9
        assert_eq!(f.hidden_ranges(), &[(3, 5), (9, 9)]);
        assert_eq!(f.row_count(12), 8);
        let rows: Vec<usize> = (0..12).map(|l| f.row_of(l)).collect();
        assert_eq!(rows, [0, 1, 2, 2, 2, 2, 3, 4, 5, 5, 6, 7]);
        let back: Vec<usize> = (0..8).map(|r| f.line_of(r)).collect();
        assert_eq!(back, [0, 1, 2, 6, 7, 8, 10, 11]);
        assert!(f.is_hidden(4) && !f.is_hidden(2) && !f.is_hidden(6));
        // Nested folds merge.
        f.fold((1, 6));
        assert_eq!(f.hidden_ranges(), &[(2, 6), (9, 9)]);
        assert!(f.unfold_at(1));
        assert_eq!(f.hidden_ranges(), &[(3, 5), (9, 9)]);
    }

    #[test]
    fn folds_follow_edits() {
        let change = |start: usize, old_end: usize, new_end: usize| Change {
            start: Point::new(start, 0),
            old_end: Point::new(old_end, 0),
            new_end: Point::new(new_end, 0),
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
        let mut f = Folds::default();
        f.fold((10, 20));
        f.shift(&change(2, 2, 4)); // two lines inserted above
        assert_eq!(f.folded(), &[(12, 22)]);
        f.shift(&change(12, 12, 12)); // typing on the first line keeps it
        assert_eq!(f.folded(), &[(12, 22)]);
        f.shift(&change(15, 15, 15)); // editing inside unfolds
        assert!(f.is_empty());
        f.fold((12, 22));
        f.shift(&change(12, 12, 13)); // Enter on the first line unfolds
        assert!(f.is_empty());
    }
}
