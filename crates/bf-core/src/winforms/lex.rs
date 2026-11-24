//! Just enough C# lexing for designer code: skipping strings and comments, matching braces, splitting a
//! method body into statements and finding identifiers.

use std::ops::Range;

/// One piece of a method body: a statement (up to and including its `;`) or a line comment between
/// statements (without the `//`).
#[derive(Debug, PartialEq)]
pub(crate) enum Item {
    Stmt(Range<usize>),
    Comment(String),
}

/// If a string, character literal or comment starts at `i`, the index right after it.
fn skip_literal(b: &[u8], i: usize) -> Option<usize> {
    let at = |k: usize| b.get(k).copied().unwrap_or(0);
    match at(i) {
        b'/' if at(i + 1) == b'/' => Some(b[i..].iter().position(|&c| c == b'\n').map_or(b.len(), |p| i + p)),
        b'/' if at(i + 1) == b'*' => Some(b[i + 2..].windows(2).position(|w| w == b"*/").map_or(b.len(), |p| i + 2 + p + 2)),
        // Verbatim strings (@"…", $@"…", @$"…"): "" is a quote.
        b'@' | b'$' if at(i + 1) == b'"' || (matches!(at(i + 1), b'@' | b'$') && at(i + 2) == b'"') => {
            let verbatim = at(i) == b'@' || at(i + 1) == b'@';
            let start = if at(i + 1) == b'"' { i + 2 } else { i + 3 };
            Some(end_of_string(b, start, verbatim))
        }
        b'"' => Some(end_of_string(b, i + 1, false)),
        b'\'' => {
            let mut k = i + 1;
            while k < b.len() && b[k] != b'\'' && b[k] != b'\n' {
                k += if b[k] == b'\\' { 2 } else { 1 };
            }
            Some((k + 1).min(b.len()))
        }
        _ => None,
    }
}

fn end_of_string(b: &[u8], mut k: usize, verbatim: bool) -> usize {
    while k < b.len() {
        match b[k] {
            b'\\' if !verbatim => k += 2,
            b'"' if verbatim && b.get(k + 1) == Some(&b'"') => k += 2,
            b'"' => return k + 1,
            b'\n' if !verbatim => return k,
            _ => k += 1,
        }
    }
    b.len()
}

fn is_comment(b: &[u8], i: usize) -> bool {
    b[i] == b'/' && matches!(b.get(i + 1), Some(b'/') | Some(b'*'))
}

/// The index of the `}` that closes the `{` at `open`.
pub(crate) fn matching_brace(text: &str, open: usize) -> Option<usize> {
    let b = text.as_bytes();
    let mut depth = 0usize;
    let mut i = open;
    while i < b.len() {
        if let Some(next) = skip_literal(b, i) {
            i = next;
            continue;
        }
        match b[i] {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Splits a method body into statements and the line comments between them.
pub(crate) fn items(body: &str) -> Vec<Item> {
    let b = body.as_bytes();
    let mut out = Vec::new();
    let (mut i, mut depth, mut start) = (0usize, 0i32, None::<usize>);
    while i < b.len() {
        if start.is_none() && b[i] == b'/' && b.get(i + 1) == Some(&b'/') {
            let end = b[i..].iter().position(|&c| c == b'\n').map_or(b.len(), |p| i + p);
            out.push(Item::Comment(body[i + 2..end].trim_end_matches('\r').to_owned()));
            i = end;
            continue;
        }
        if let Some(next) = skip_literal(b, i) {
            if start.is_none() && !is_comment(b, i) {
                start = Some(i);
            }
            i = next;
            continue;
        }
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if start.is_none() {
            start = Some(i);
        }
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b';' if depth <= 0 => {
                out.push(Item::Stmt(start.take().unwrap_or(i)..i + 1));
                depth = 0;
            }
            _ => {}
        }
        i += 1;
    }
    if let Some(s) = start {
        let end = body.trim_end().len().max(s);
        out.push(Item::Stmt(s..end));
    }
    out
}

/// Index of the first `ch` in `s` outside strings, comments and brackets.
pub(crate) fn find_top(s: &str, ch: u8) -> Option<usize> {
    let b = s.as_bytes();
    let (mut i, mut depth) = (0usize, 0i32);
    while i < b.len() {
        if let Some(next) = skip_literal(b, i) {
            i = next;
            continue;
        }
        match b[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            c if c == ch && depth == 0 => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

/// The top-level, comma-separated arguments of an argument list (without its parentheses).
pub(crate) fn split_args(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(p) = find_top(rest, b',') {
        out.push(rest[..p].trim());
        rest = &rest[p + 1..];
    }
    if !rest.trim().is_empty() || !out.is_empty() {
        out.push(rest.trim());
    }
    out
}

/// The text between the first `(` (outside strings) and its `)`.
pub(crate) fn call_args(s: &str) -> Option<&str> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if let Some(next) = skip_literal(b, i) {
            i = next;
            continue;
        }
        if b[i] == b'(' {
            return matching_paren(s, i).map(|close| &s[i + 1..close]);
        }
        i += 1;
    }
    None
}

/// The index of the `)` that closes the `(` at `open`.
pub(crate) fn matching_paren(s: &str, open: usize) -> Option<usize> {
    let b = s.as_bytes();
    let (mut i, mut depth) = (open, 0i32);
    while i < b.len() {
        if let Some(next) = skip_literal(b, i) {
            i = next;
            continue;
        }
        match b[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

pub(crate) fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

pub fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_alphabetic() || c == '_') && chars.all(is_ident_char) && !KEYWORDS.contains(&s)
}

const KEYWORDS: &[&str] = &[
    "abstract", "as", "base", "bool", "break", "byte", "case", "catch", "char", "checked", "class", "const", "continue", "decimal", "default",
    "delegate", "do", "double", "else", "enum", "event", "explicit", "extern", "false", "finally", "fixed", "float", "for", "foreach", "goto",
    "if", "implicit", "in", "int", "interface", "internal", "is", "lock", "long", "namespace", "new", "null", "object", "operator", "out",
    "override", "params", "private", "protected", "public", "readonly", "ref", "return", "sbyte", "sealed", "short", "sizeof", "stackalloc",
    "static", "string", "struct", "switch", "this", "throw", "true", "try", "typeof", "uint", "ulong", "unchecked", "unsafe", "ushort",
    "using", "virtual", "void", "volatile", "while",
];

/// The identifiers of `s` outside strings and comments, with their byte ranges; `member` says whether one
/// follows a `.` (other than `this.`).
pub(crate) fn identifiers(s: &str) -> Vec<(Range<usize>, bool)> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if let Some(next) = skip_literal(b, i) {
            i = next;
            continue;
        }
        let c = s[i..].chars().next().unwrap_or(' ');
        if c.is_alphabetic() || c == '_' {
            let end = s[i..].char_indices().find(|(_, c)| !is_ident_char(*c)).map_or(s.len(), |(k, _)| i + k);
            let before = s[..i].trim_end();
            let member = before.ends_with('.') && !before.ends_with("this.");
            out.push((i..end, member));
            i = end;
        } else if c.is_ascii_digit() {
            // A number (with suffixes like 9F), not an identifier.
            i += s[i..].char_indices().find(|(_, c)| !is_ident_char(*c) && *c != '.').map_or(s.len() - i, |(k, _)| k);
        } else {
            i += c.len_utf8();
        }
    }
    out
}

/// `s` with every free-standing use of `old` (not a member of something else, not in a string) renamed.
pub(crate) fn replace_ident(s: &str, old: &str, new: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    for (range, member) in identifiers(s) {
        if !member && &s[range.clone()] == old {
            out.push_str(&s[last..range.start]);
            out.push_str(new);
            last = range.end;
        }
    }
    out.push_str(&s[last..]);
    out
}

/// Whether `s` uses `name` as a free-standing identifier.
pub(crate) fn mentions(s: &str, name: &str) -> bool {
    identifiers(s).into_iter().any(|(r, member)| !member && &s[r] == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statements_and_comments() {
        let body = "a = \"x;y\"; // c\n// \n// button1\n// \nb.Items.AddRange(new object[] {\n \"1;\", @\"q\"\"\" });\nc();";
        let items = items(body);
        let texts: Vec<String> = items
            .iter()
            .map(|i| match i {
                Item::Stmt(r) => format!("S:{}", &body[r.clone()]),
                Item::Comment(c) => format!("C:{c}"),
            })
            .collect();
        assert_eq!(texts, ["S:a = \"x;y\";", "C: c", "C: ", "C: button1", "C: ", "S:b.Items.AddRange(new object[] {\n \"1;\", @\"q\"\"\" });", "S:c();"]);
    }

    #[test]
    fn braces_skip_strings_and_comments() {
        let text = "{ \"}\" /* } */ '}' // }\n { } }";
        assert_eq!(matching_brace(text, 0), Some(text.len() - 1));
    }

    #[test]
    fn renames_free_identifiers_only() {
        assert_eq!(replace_ident("this.button1.Text = button1.Name + \"button1\" + x.button1;", "button1", "ok"), "this.ok.Text = ok.Name + \"button1\" + x.button1;");
        assert_eq!(split_args("1, f(2, 3), \"a,b\""), ["1", "f(2, 3)", "\"a,b\""]);
        assert!(is_identifier("button_1") && !is_identifier("1a") && !is_identifier("class"));
    }
}
