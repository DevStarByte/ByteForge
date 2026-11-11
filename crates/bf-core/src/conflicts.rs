//! Merge conflicts in a file: the `<<<<<<<` / `=======` / `>>>>>>>` blocks git leaves (with the
//! `|||||||` base section of the diff3 style), and how each is resolved.

/// One conflict, as 0-based line numbers of its markers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    /// `<<<<<<< HEAD`: the current (ours) side follows.
    pub start: usize,
    /// `||||||| base` (diff3 style), when present.
    pub base: Option<usize>,
    /// `=======`: the incoming (theirs) side follows.
    pub separator: usize,
    /// `>>>>>>> branch`.
    pub end: usize,
    /// What the markers name ("HEAD", "feature/x").
    pub current_label: String,
    pub incoming_label: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    Current,
    Incoming,
    /// Current, then incoming.
    Both,
}

fn marker<'a>(line: &'a str, m: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(m)?;
    // Exactly seven characters, then the end or a space (a longer run is not a marker).
    (rest.is_empty() || rest.starts_with(' ')).then(|| rest.trim())
}

/// The conflicts in `text`, in order. Unfinished blocks are left out.
pub fn find(text: &str) -> Vec<Conflict> {
    if !text.contains("<<<<<<<") {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut open: Option<(usize, String, Option<usize>, Option<usize>)> = None;
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim_end_matches('\r');
        if let Some(label) = marker(line, "<<<<<<<") {
            open = Some((i, label.to_owned(), None, None));
        } else if let Some((_, _, base, sep)) = open.as_mut() {
            if marker(line, "|||||||").is_some() && sep.is_none() {
                *base = Some(i);
            } else if line.trim_end() == "=======" && sep.is_none() {
                *sep = Some(i);
            } else if let Some(label) = marker(line, ">>>>>>>")
                && let Some((start, current_label, base, Some(separator))) = open.take()
            {
                out.push(Conflict { start, base, separator, end: i, current_label, incoming_label: label.to_owned() });
            }
        }
    }
    out
}

/// The lines that replace the conflict's block (`start ..= end`).
pub fn resolve<'a>(lines: &[&'a str], c: &Conflict, how: Resolution) -> Vec<&'a str> {
    let current = &lines[c.start + 1..c.base.unwrap_or(c.separator)];
    let incoming = &lines[c.separator + 1..c.end];
    match how {
        Resolution::Current => current.to_vec(),
        Resolution::Incoming => incoming.to_vec(),
        Resolution::Both => current.iter().chain(incoming).copied().collect(),
    }
}

/// `text` with the conflict replaced by the side(s) kept (line endings and the final newline preserved).
pub fn apply(text: &str, c: &Conflict, how: Resolution) -> String {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let lines: Vec<&str> = text.lines().collect();
    let kept = resolve(&lines, c, how);
    let mut out: Vec<&str> = Vec::with_capacity(lines.len());
    out.extend_from_slice(&lines[..c.start]);
    out.extend(kept);
    out.extend_from_slice(&lines[c.end + 1..]);
    let mut joined = out.join(newline);
    if text.ends_with('\n') && !joined.is_empty() {
        joined.push_str(newline);
    }
    joined
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "a\n<<<<<<< HEAD\nours 1\nours 2\n=======\ntheirs\n>>>>>>> feature/x\nb\n<<<<<<< HEAD\nx\n||||||| base\nold\n=======\ny\n>>>>>>> other\n";

    #[test]
    fn finds_conflicts_with_and_without_base() {
        let found = find(TEXT);
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].start, found[0].separator, found[0].end, found[0].base), (1, 4, 6, None));
        assert_eq!((found[0].current_label.as_str(), found[0].incoming_label.as_str()), ("HEAD", "feature/x"));
        assert_eq!((found[1].start, found[1].base, found[1].separator, found[1].end), (8, Some(10), 12, 14));
    }

    #[test]
    fn resolves_each_way() {
        let lines: Vec<&str> = TEXT.lines().collect();
        let found = find(TEXT);
        assert_eq!(resolve(&lines, &found[0], Resolution::Current), ["ours 1", "ours 2"]);
        assert_eq!(resolve(&lines, &found[0], Resolution::Incoming), ["theirs"]);
        assert_eq!(resolve(&lines, &found[0], Resolution::Both), ["ours 1", "ours 2", "theirs"]);
        // The base section is dropped.
        assert_eq!(resolve(&lines, &found[1], Resolution::Both), ["x", "y"]);
    }

    #[test]
    fn applies_a_resolution_to_the_text() {
        let found = find(TEXT);
        let once = apply(TEXT, &found[0], Resolution::Incoming);
        assert!(once.starts_with("a\ntheirs\nb\n<<<<<<< HEAD"));
        let again = find(&once);
        assert_eq!(again.len(), 1);
        assert_eq!(apply(&once, &again[0], Resolution::Current), "a\ntheirs\nb\nx\n");
        let crlf = "a\r\n<<<<<<< HEAD\r\nx\r\n=======\r\n>>>>>>> b\r\nz\r\n";
        assert_eq!(apply(crlf, &find(crlf)[0], Resolution::Incoming), "a\r\nz\r\n");
    }

    #[test]
    fn ignores_lookalikes_and_unfinished_blocks() {
        assert!(find("<<<<<<<< not a marker\n=======\n>>>>>>> x\n").is_empty());
        assert!(find("<<<<<<< HEAD\nno end\n").is_empty());
        assert!(find("plain text\n").is_empty());
    }
}
