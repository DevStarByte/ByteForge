//! What Find (in a file) and Search (across files) look for: plain text or a regular expression, with
//! match case and whole word. Matches are found line by line.

use regex::{Regex, RegexBuilder};

/// Search options as the find bar and the Search view offer them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    pub text: String,
    pub match_case: bool,
    pub whole_word: bool,
    pub regex: bool,
}

/// A compiled [`Query`].
#[derive(Clone, Debug)]
pub struct Matcher {
    re: Regex,
    whole_word: bool,
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Matcher {
    /// `None` for an empty query; an error message for an invalid regular expression.
    pub fn new(query: &Query) -> Result<Option<Matcher>, String> {
        if query.text.is_empty() {
            return Ok(None);
        }
        let pattern = if query.regex { query.text.clone() } else { regex::escape(&query.text) };
        let re = RegexBuilder::new(&pattern)
            .case_insensitive(!query.match_case)
            .size_limit(4 << 20)
            .build()
            .map_err(|e| match e {
                regex::Error::Syntax(text) => text.lines().last().unwrap_or("invalid regular expression").trim().to_owned(),
                other => other.to_string(),
            })?;
        Ok(Some(Matcher { re, whole_word: query.whole_word }))
    }

    /// The matches in `line` as byte ranges, left to right. Empty matches (`^`, `a*`) are left out.
    pub fn find_in(&self, line: &str) -> Vec<(usize, usize)> {
        self.re
            .find_iter(line)
            .map(|m| (m.start(), m.end()))
            .filter(|&(s, e)| e > s)
            .filter(|&(s, e)| !self.whole_word || (!line[..s].chars().next_back().is_some_and(is_word) && !line[e..].chars().next().is_some_and(is_word)))
            .collect()
    }

    /// The text that replaces the match at `start..end` of `line`: `template` with `$1`, `${name}` and `$$`
    /// filled in for a regular expression (the whole match is `$0`).
    pub fn replacement(&self, line: &str, start: usize, template: &str) -> String {
        match self.re.captures_at(line, start) {
            Some(caps) if caps.get(0).is_some_and(|m| m.start() == start) => {
                let mut out = String::new();
                caps.expand(template, &mut out);
                out
            }
            _ => template.to_owned(),
        }
    }
}

/// Converts a byte range in `line` to a char range.
pub fn char_range(line: &str, (start, end): (usize, usize)) -> (usize, usize) {
    let s = line[..start].chars().count();
    (s, s + line[start..end].chars().count())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(text: &str) -> Query {
        Query { text: text.into(), ..Default::default() }
    }

    #[test]
    fn plain_text_is_not_a_pattern() {
        let m = Matcher::new(&q("a.b")).unwrap().unwrap();
        assert_eq!(m.find_in("axb a.b"), [(4, 7)]);
        assert!(Matcher::new(&q("")).unwrap().is_none());
    }

    #[test]
    fn match_case_and_whole_word() {
        let m = Matcher::new(&q("count")).unwrap().unwrap();
        assert_eq!(m.find_in("Count counter count"), [(0, 5), (6, 11), (14, 19)]);
        let m = Matcher::new(&Query { match_case: true, whole_word: true, ..q("count") }).unwrap().unwrap();
        assert_eq!(m.find_in("Count counter count _count"), [(14, 19)]);
        // A query that starts or ends with a symbol still needs a non-word character next to it.
        let m = Matcher::new(&Query { whole_word: true, ..q("x.") }).unwrap().unwrap();
        assert_eq!(m.find_in("ax. x. x.y"), [(4, 6)]);
    }

    #[test]
    fn regular_expressions_and_captures() {
        let m = Matcher::new(&Query { regex: true, ..q(r"(\w+)\s*=\s*(\d+)") }).unwrap().unwrap();
        let line = "var total = 42;";
        let found = m.find_in(line);
        assert_eq!(found, [(4, 14)]);
        assert_eq!(m.replacement(line, found[0].0, "$1 += $2"), "total += 42");
        assert_eq!(m.replacement(line, found[0].0, "${1}_$$"), "total_$");
        assert!(Matcher::new(&Query { regex: true, ..q("(") }).unwrap_err().contains("unclosed"));
        // Empty matches don't count.
        let m = Matcher::new(&Query { regex: true, ..q("x*") }).unwrap().unwrap();
        assert_eq!(m.find_in("abxxc"), [(2, 4)]);
    }

    #[test]
    fn char_ranges_for_non_ascii_lines() {
        let line = "ü = größe";
        let m = Matcher::new(&q("größe")).unwrap().unwrap();
        let found = m.find_in(line)[0];
        assert_eq!(char_range(line, found), (4, 9));
        // Case-insensitive matching works across Unicode case.
        assert_eq!(Matcher::new(&q("GRÖSSE")).unwrap().unwrap().find_in("größe").len(), 0, "ß doesn't fold to SS");
        assert_eq!(Matcher::new(&q("GRÖßE")).unwrap().unwrap().find_in("größe").len(), 1);
    }
}
