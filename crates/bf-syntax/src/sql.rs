//! SQL highlighting with a small lexer instead of a tree-sitter grammar. The SQL grammars on crates.io
//! carry a parser.c of about 47 MB, which MSVC can't compile (the Windows build failed on it); SQL is
//! regular enough that tokens are all highlighting needs: `--` and `/* */` comments (across lines),
//! strings with `''` escapes, quoted identifiers, numbers, keywords, built-in types, function calls,
//! `@variables` / `:params` / `$1`, and operators. Dialect-neutral: T-SQL, PostgreSQL, MySQL, SQLite.

use std::ops::Range;

use crate::Span;

const KEYWORDS: &[&str] = &[
    "ADD", "ALL", "ALTER", "AND", "ANY", "AS", "ASC", "AUTHORIZATION", "AUTO_INCREMENT", "AUTOINCREMENT", "BACKUP", "BEGIN", "BETWEEN", "BREAK", "BY",
    "CASCADE", "CASE", "CATCH", "CHECK", "CLOSE", "CLUSTERED", "COLLATE", "COLUMN", "COMMIT", "CONFLICT", "CONSTRAINT", "CONTINUE", "CREATE", "CROSS",
    "CURRENT", "CURSOR", "DATABASE", "DEALLOCATE", "DECLARE", "DEFAULT", "DELETE", "DESC", "DISTINCT", "DO", "DROP", "EACH", "ELSE", "END", "ESCAPE",
    "EXCEPT", "EXEC", "EXECUTE", "EXISTS", "EXPLAIN", "FALSE", "FETCH", "FILTER", "FIRST", "FOLLOWING", "FOR", "FOREIGN", "FROM", "FULL", "FUNCTION",
    "GO", "GOTO", "GRANT", "GROUP", "HAVING", "IDENTITY", "IF", "ILIKE", "IN", "INDEX", "INNER", "INSERT", "INTERSECT", "INTO", "IS", "JOIN", "KEY",
    "LANGUAGE", "LAST", "LATERAL", "LEFT", "LIKE", "LIMIT", "MATERIALIZED", "MERGE", "NEXT", "NOCHECK", "NONCLUSTERED", "NOT", "NOTHING", "NULL",
    "NULLS", "OF", "OFF", "OFFSET", "ON", "ONLY", "OPEN", "OR", "ORDER", "OUTER", "OUTPUT", "OVER", "PARTITION", "PERCENT", "PRECEDING", "PRIMARY",
    "PRINT", "PROC", "PROCEDURE", "RAISERROR", "RANGE", "RECURSIVE", "REFERENCES", "RENAME", "REPLACE", "RESTRICT", "RETURN", "RETURNING", "RETURNS",
    "REVOKE", "RIGHT", "ROLLBACK", "ROW", "ROWS", "SAVEPOINT", "SCHEMA", "SELECT", "SEQUENCE", "SET", "SOME", "TABLE", "TEMP", "TEMPORARY", "THEN",
    "THROW", "TIES", "TO", "TOP", "TRAN", "TRANSACTION", "TRIGGER", "TRUE", "TRUNCATE", "TRY", "UNBOUNDED", "UNION", "UNIQUE", "UNKNOWN", "UPDATE",
    "USE", "USING", "VALUES", "VIEW", "WAITFOR", "WHEN", "WHERE", "WHILE", "WINDOW", "WITH", "WITHOUT",
];

const TYPES: &[&str] = &[
    "BIGINT", "BIGSERIAL", "BINARY", "BIT", "BLOB", "BOOL", "BOOLEAN", "BYTEA", "CHAR", "CHARACTER", "CITEXT", "CLOB", "DATE", "DATETIME", "DATETIME2",
    "DATETIMEOFFSET", "DEC", "DECIMAL", "DOUBLE", "FLOAT", "GEOGRAPHY", "GEOMETRY", "IMAGE", "INT", "INTEGER", "INTERVAL", "JSON", "JSONB", "MONEY",
    "NCHAR", "NTEXT", "NUMERIC", "NVARCHAR", "PRECISION", "REAL", "ROWVERSION", "SERIAL", "SMALLDATETIME", "SMALLINT", "SMALLMONEY", "TEXT", "TIME",
    "TIMESTAMP", "TIMESTAMPTZ", "TINYINT", "UNIQUEIDENTIFIER", "UUID", "VARBINARY", "VARCHAR", "VARYING", "XML", "ZONE",
];

fn is_word(list: &[&str], word: &str) -> bool {
    list.iter().any(|k| k.eq_ignore_ascii_case(word))
}

/// The spans of `text` intersecting `range`, sorted and non-overlapping, clipped to it. Lexes from the
/// start of the text (a block comment or string can begin above the range).
pub(crate) fn highlight(text: &str, range: Range<usize>) -> Vec<Span> {
    let end = range.end.min(text.len());
    let start = range.start.min(end);
    let b = text.as_bytes();
    let mut spans = Vec::new();
    let mut push = |s: usize, e: usize, kind: &'static str| {
        let (s, e) = (s.max(start), e.min(end));
        if s < e {
            spans.push(Span { start: s, end: e, kind });
        }
    };
    let mut i = 0;
    while i < end {
        let c = b[i];
        let next = b.get(i + 1).copied().unwrap_or(0);
        let t = i;
        if c == b'-' && next == b'-' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            push(t, i, "comment");
        } else if c == b'/' && next == b'*' {
            i += 2;
            while i < b.len() && !(b[i] == b'*' && b.get(i + 1) == Some(&b'/')) {
                i += 1;
            }
            i = (i + 2).min(b.len());
            push(t, i, "comment");
        } else if c == b'\'' || ((c == b'N' || c == b'n' || c == b'E' || c == b'e') && next == b'\'') {
            // 'it''s', N'unicode', E'escapes'
            i += if c == b'\'' { 1 } else { 2 };
            while i < b.len() {
                if b[i] == b'\'' {
                    if b.get(i + 1) == Some(&b'\'') {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                if b[i] == b'\\' && (c == b'E' || c == b'e') {
                    i += 1;
                }
                i += 1;
            }
            push(t, i.min(b.len()), "string");
        } else if c == b'$' && (next == b'$' || next.is_ascii_alphabetic() || next == b'_') && dollar_tag(b, i).is_some() {
            // PostgreSQL $$ … $$ / $tag$ … $tag$ bodies.
            let tag = dollar_tag(b, i).unwrap_or(2);
            let close = &text[i..i + tag];
            i += tag;
            i = text[i..].find(close).map_or(b.len(), |p| i + p + tag);
            push(t, i, "string");
        } else if c == b'"' || c == b'[' || c == b'`' {
            // Quoted identifiers: "name", [name], `name`.
            let close = if c == b'[' { b']' } else { c };
            i += 1;
            while i < b.len() && b[i] != close && b[i] != b'\n' {
                i += 1;
            }
            i = (i + 1).min(b.len());
        } else if c.is_ascii_digit() || (c == b'.' && next.is_ascii_digit()) {
            if c == b'0' && (next == b'x' || next == b'X') {
                i += 2;
                while i < b.len() && b[i].is_ascii_hexdigit() {
                    i += 1;
                }
            } else {
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                    i += 1;
                }
                if i < b.len() && (b[i] == b'e' || b[i] == b'E') && b.get(i + 1).is_some_and(|d| d.is_ascii_digit() || *d == b'-' || *d == b'+') {
                    i += 2;
                    while i < b.len() && b[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            push(t, i, "number");
        } else if c == b'@' || (c == b':' && next.is_ascii_alphabetic() && b.get(i.wrapping_sub(1)) != Some(&b':')) || (c == b'$' && next.is_ascii_digit()) {
            // @var, @@ROWCOUNT, :param, $1
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'@') {
                i += 1;
            }
            push(t, i, "parameter name");
        } else if c.is_ascii_alphabetic() || c == b'_' || c >= 0x80 {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'$' || b[i] >= 0x80) {
                i += 1;
            }
            let word = &text[t..i];
            let after = text[i..].trim_start_matches([' ', '\t']);
            // Built-in types color like keywords (as `type.builtin` does for the tree-sitter languages).
            if is_word(TYPES, word) || is_word(KEYWORDS, word) {
                push(t, i, "keyword");
            } else if after.starts_with('(') {
                push(t, i, "method name");
            }
        } else if b"=<>!+-*/%|&^~".contains(&c) {
            while i < b.len() && b"=<>!|&".contains(&b[i]) && i - t < 3 {
                i += 1;
            }
            i = i.max(t + 1);
            push(t, i, "operator");
        } else {
            // Advance by a whole character.
            i += text[i..].chars().next().map_or(1, char::len_utf8);
        }
        debug_assert!(i > t);
    }
    spans
}

/// The length of a PostgreSQL dollar-quote tag at `i` (`$$` or `$name$`).
fn dollar_tag(b: &[u8], i: usize) -> Option<usize> {
    let mut j = i + 1;
    while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
        j += 1;
    }
    (b.get(j) == Some(&b'$')).then_some(j + 1 - i)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(text: &str, needle: &str) -> Option<&'static str> {
        let at = text.find(needle).unwrap();
        highlight(text, 0..text.len()).into_iter().find(|s| s.start <= at && at < s.end).map(|s| s.kind)
    }

    #[test]
    fn lexes_tokens_of_several_dialects() {
        let sql = "/* multi\nline */\nDECLARE @n INT = 0x1F;\nselect count(*), [Order Id], \"q\" from t where name = N'it''s' and x >= 1.5e3 -- done\n";
        assert_eq!(kind(sql, "line */"), Some("comment"), "a block comment across lines");
        assert_eq!(kind(sql, "DECLARE"), Some("keyword"));
        assert_eq!(kind(sql, "@n"), Some("parameter name"));
        assert_eq!(kind(sql, "INT"), Some("keyword"));
        assert_eq!(kind(sql, "0x1F"), Some("number"));
        assert_eq!(kind(sql, "select"), Some("keyword"), "any case");
        assert_eq!(kind(sql, "count"), Some("method name"));
        assert_eq!(kind(sql, "Order Id"), None, "a quoted identifier isn't a keyword");
        assert_eq!(kind(sql, "t''s"), Some("string"), "'' stays in the string");
        assert_eq!(kind(sql, ">="), Some("operator"));
        assert_eq!(kind(sql, "1.5e3"), Some("number"));
        assert_eq!(kind(sql, "done"), Some("comment"));
        let pg = "CREATE FUNCTION f() RETURNS int AS $$ SELECT 1 $$ LANGUAGE sql; SELECT $1, :name::text;";
        assert_eq!(kind(pg, " SELECT 1 "), Some("string"));
        assert_eq!(kind(pg, "$1"), Some("parameter name"));
        assert_eq!(kind(pg, ":name"), Some("parameter name"));
    }

    #[test]
    fn clips_to_the_range_and_survives_unclosed_tokens() {
        let sql = "SELECT 'abc\n/* open";
        let mid = sql.find("bc").unwrap();
        let spans = highlight(sql, mid..sql.len());
        assert!(spans.iter().all(|s| s.start >= mid && s.end <= sql.len() && s.start < s.end));
        assert_eq!(spans[0].kind, "string");
        for end in 0..=sql.len() {
            if sql.is_char_boundary(end) {
                highlight(sql, 0..end);
            }
        }
        highlight("ünïcode SELECT", 0..15);
    }
}
