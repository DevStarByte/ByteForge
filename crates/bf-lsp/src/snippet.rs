//! Conversion of LSP snippet syntax to plain text.

/// Strips snippet syntax: `$1`/`$0` and variables are removed, `${1:text}` becomes `text`
/// (recursively), `${1|a,b|}` becomes `a`, and `\$`, `\}`, `\\` escapes are unescaped.
pub fn strip_snippet(snippet: &str) -> String {
    let chars: Vec<char> = snippet.chars().collect();
    let mut out = String::with_capacity(snippet.len());
    let mut i = 0;
    parse(&chars, &mut i, &mut out, false);
    out
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn parse(c: &[char], i: &mut usize, out: &mut String, nested: bool) {
    while *i < c.len() {
        match c[*i] {
            '\\' if matches!(c.get(*i + 1), Some('$' | '}' | '\\')) => {
                out.push(c[*i + 1]);
                *i += 2;
            }
            '}' if nested => return,
            '$' => match c.get(*i + 1) {
                Some(&n) if n.is_ascii_digit() || n == '_' || n.is_ascii_alphabetic() => {
                    *i += 1;
                    while *i < c.len() && is_name_char(c[*i]) {
                        *i += 1;
                    }
                }
                Some('{') => {
                    let open = *i;
                    *i += 2;
                    while *i < c.len() && is_name_char(c[*i]) {
                        *i += 1;
                    }
                    match c.get(*i) {
                        Some('}') => *i += 1,
                        Some(':') => {
                            *i += 1;
                            parse(c, i, out, true);
                            if c.get(*i) == Some(&'}') {
                                *i += 1;
                            }
                        }
                        Some('|') => {
                            *i += 1;
                            out.push_str(&parse_choice(c, i));
                        }
                        _ => {
                            // Not a construct we understand: keep it literally.
                            out.extend(&c[open..*i]);
                        }
                    }
                }
                _ => {
                    out.push('$');
                    *i += 1;
                }
            },
            ch => {
                out.push(ch);
                *i += 1;
            }
        }
    }
}

/// Parses `a,b,c|}` (after the opening `|`) and returns the first choice.
fn parse_choice(c: &[char], i: &mut usize) -> String {
    let mut first = String::new();
    let mut in_first = true;
    while *i < c.len() {
        match c[*i] {
            '\\' if *i + 1 < c.len() => {
                if in_first {
                    first.push(c[*i + 1]);
                }
                *i += 2;
                continue;
            }
            '|' if c.get(*i + 1) == Some(&'}') => {
                *i += 2;
                break;
            }
            ',' => in_first = false,
            ch if in_first => first.push(ch),
            _ => {}
        }
        *i += 1;
    }
    first
}

#[cfg(test)]
mod tests {
    use super::strip_snippet;

    #[test]
    fn tabstops_and_placeholders() {
        assert_eq!(strip_snippet("WriteLine($1)$0"), "WriteLine()");
        assert_eq!(strip_snippet("Foo(${1:int x}, ${2:y})"), "Foo(int x, y)");
        assert_eq!(strip_snippet("${1:outer ${2:inner}}"), "outer inner");
        assert_eq!(strip_snippet("${1}"), "");
    }

    #[test]
    fn choices_and_escapes() {
        assert_eq!(strip_snippet("${1|public,private|} class"), "public class");
        assert_eq!(strip_snippet(r"cost: \$5 \} \\"), r"cost: $5 } \");
        assert_eq!(strip_snippet("${1:a\\}b}"), "a}b");
    }

    #[test]
    fn plain_text_untouched() {
        assert_eq!(strip_snippet("Console"), "Console");
        assert_eq!(strip_snippet("a $ b"), "a $ b");
        assert_eq!(strip_snippet("x$"), "x$");
        assert_eq!(strip_snippet("${TM/a/b/}"), "${TM/a/b/}");
    }
}
