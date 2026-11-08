//! Code snippets in VS Code's format: built-in C# snippets plus the user's
//! `<config>/snippets/<language>.json` (`{ "Name": { "prefix": "prop", "body": [...], "description": "…" } }`),
//! and expansion of a snippet body into text with tab stops.
//!
//! Supported syntax: `$1`, `${1}`, `${1:default}` (defaults may contain variables), `${1|one,two|}` (the
//! first choice), `$0` (final caret), variables `$TM_FILENAME_BASE` / `${TM_FILENAME_BASE:default}` and
//! a few others, and `\$`, `\}`, `\\` escapes.

use std::collections::BTreeMap;
use std::ops::Range;
use std::path::Path;

use serde::Deserialize;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snippet {
    pub prefix: String,
    pub body: String,
    pub description: String,
}

/// An expanded snippet: the text to insert and its tab stops as char offsets into `text`, in visiting
/// order (`$1`, `$2`, …, then `$0`, which is the end of the text when the body has none). Each stop lists
/// every occurrence: the first is the placeholder, the rest are mirrors (`for (int ${1:i} = 0; $1 < …`)
/// that start with the same text and should follow it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expansion {
    pub text: String,
    pub stops: Vec<Vec<Range<usize>>>,
}

/// Built-in C# snippets (the ones Visual Studio users expect).
pub fn builtin_csharp() -> Vec<Snippet> {
    let s = |prefix: &str, description: &str, body: &str| Snippet { prefix: prefix.into(), body: body.into(), description: description.into() };
    vec![
        s("prop", "Auto-implemented property", "public ${1:int} ${2:MyProperty} { get; set; }$0"),
        s("propg", "Get-only property", "public ${1:int} ${2:MyProperty} { get; }$0"),
        s("propi", "Init-only property", "public ${1:int} ${2:MyProperty} { get; init; }$0"),
        s("propfull", "Property with a backing field", "private ${1:int} ${2:myVar};\n\npublic $1 ${3:MyProperty}\n{\n\tget => $2;\n\tset => $2 = value;\n}$0"),
        s("ctor", "Constructor", "public ${1:$TM_FILENAME_BASE}($2)\n{\n\t$0\n}"),
        s("class", "Class", "public class ${1:$TM_FILENAME_BASE}\n{\n\t$0\n}"),
        s("record", "Record", "public record ${1:$TM_FILENAME_BASE}($2);$0"),
        s("struct", "Struct", "public struct ${1:MyStruct}\n{\n\t$0\n}"),
        s("interface", "Interface", "public interface ${1:I$TM_FILENAME_BASE}\n{\n\t$0\n}"),
        s("enum", "Enum", "public enum ${1:MyEnum}\n{\n\t$0\n}"),
        s("cw", "Console.WriteLine", "Console.WriteLine($1);$0"),
        s("if", "if statement", "if (${1:true})\n{\n\t$0\n}"),
        s("else", "else statement", "else\n{\n\t$0\n}"),
        s("for", "for loop", "for (int ${1:i} = 0; $1 < ${2:length}; $1++)\n{\n\t$0\n}"),
        s("forr", "Reverse for loop", "for (int ${1:i} = ${2:length} - 1; $1 >= 0; $1--)\n{\n\t$0\n}"),
        s("foreach", "foreach loop", "foreach (var ${1:item} in ${2:collection})\n{\n\t$0\n}"),
        s("while", "while loop", "while (${1:true})\n{\n\t$0\n}"),
        s("do", "do-while loop", "do\n{\n\t$0\n} while (${1:true});"),
        s("switch", "switch statement", "switch (${1:value})\n{\n\tcase ${2:0}:\n\t\t$0\n\t\tbreak;\n\tdefault:\n\t\tbreak;\n}"),
        s("try", "try-catch", "try\n{\n\t$1\n}\ncatch (${2:Exception} ${3:ex})\n{\n\t$0\n}"),
        s("tryf", "try-finally", "try\n{\n\t$1\n}\nfinally\n{\n\t$0\n}"),
        s("using", "using statement", "using (${1:var resource = ...})\n{\n\t$0\n}"),
        s("lock", "lock statement", "lock (${1:this})\n{\n\t$0\n}"),
        s("svm", "static void Main", "static void Main(string[] args)\n{\n\t$0\n}"),
        s("sim", "static int Main", "static int Main(string[] args)\n{\n\t$0\n\treturn 0;\n}"),
        s("region", "#region", "#region ${1:MyRegion}\n$0\n#endregion"),
        s("test", "xUnit test method", "[Fact]\npublic void ${1:Method_Condition_Result}()\n{\n\t$0\n}"),
        s("testasync", "Async xUnit test method", "[Fact]\npublic async Task ${1:Method_Condition_Result}()\n{\n\t$0\n}"),
        s("ex", "Exception class", "public class ${1:My}Exception : Exception\n{\n\tpublic $1Exception() { }\n\tpublic $1Exception(string message) : base(message) { }\n\tpublic $1Exception(string message, Exception inner) : base(message, inner) { }\n}$0"),
    ]
}

#[derive(Deserialize)]
struct RawSnippet {
    prefix: Option<StringOrList>,
    body: StringOrList,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StringOrList {
    One(String),
    Many(Vec<String>),
}

impl StringOrList {
    fn items(self) -> Vec<String> {
        match self {
            Self::One(s) => vec![s],
            Self::Many(v) => v,
        }
    }
}

/// Parses a VS Code snippets file (JSON with comments). A body list is joined with newlines; a prefix list
/// gives one snippet per prefix.
pub fn parse_snippets(text: &str) -> anyhow::Result<Vec<Snippet>> {
    let raw: BTreeMap<String, RawSnippet> = json5::from_str(text)?;
    let mut out = Vec::new();
    for (name, snippet) in raw {
        let body = snippet.body.items().join("\n");
        let description = snippet.description.unwrap_or_else(|| name.clone());
        let prefixes = snippet.prefix.map(StringOrList::items).unwrap_or_else(|| vec![name.clone()]);
        for prefix in prefixes.into_iter().filter(|p| !p.trim().is_empty()) {
            out.push(Snippet { prefix, body: body.clone(), description: description.clone() });
        }
    }
    Ok(out)
}

/// Built-in snippets for `language` (only "csharp" has any) plus the user's `<dir>/<language>.json`.
/// User snippets with the same prefix as a built-in one replace it.
pub fn for_language(dir: &Path, language: &str) -> Vec<Snippet> {
    let mut snippets = if language == "csharp" { builtin_csharp() } else { Vec::new() };
    let path = dir.join(format!("{language}.json"));
    if let Ok(text) = std::fs::read_to_string(&path) {
        match parse_snippets(&text) {
            Ok(user) => {
                snippets.retain(|s| !user.iter().any(|u| u.prefix == s.prefix));
                snippets.extend(user);
            }
            Err(e) => log::warn!("invalid snippets file {}: {e}", path.display()),
        }
    }
    snippets
}

/// Expands `body`: every line after the first gets `indent` (the current line's indentation), a `\t` in
/// the body becomes `tab` (the editor's indent unit), and variables come from `variable`.
pub fn expand(body: &str, indent: &str, tab: &str, variable: &dyn Fn(&str) -> Option<String>) -> Expansion {
    let mut out = Output { text: String::new(), chars: 0, stops: BTreeMap::new(), values: BTreeMap::new(), indent, tab };
    let chars: Vec<char> = body.chars().collect();
    parse(&chars, &mut 0, &mut out, variable, false);
    let end = out.chars;
    let mut stops: Vec<Vec<Range<usize>>> = out.stops.iter().filter(|(n, _)| **n != 0).map(|(_, r)| r.clone()).collect();
    // The final caret: one occurrence, an empty range.
    stops.push(out.stops.get(&0).cloned().unwrap_or_else(|| std::iter::once(end..end).collect()));
    Expansion { text: out.text, stops }
}

struct Output<'a> {
    text: String,
    chars: usize,
    /// Occurrences of each tab stop number (the first is the placeholder).
    stops: BTreeMap<u32, Vec<Range<usize>>>,
    /// The text of each placeholder, for its mirrors.
    values: BTreeMap<u32, String>,
    indent: &'a str,
    tab: &'a str,
}

impl Output<'_> {
    fn push(&mut self, c: char) {
        match c {
            '\n' => {
                self.text.push('\n');
                self.text.push_str(self.indent);
                self.chars += 1 + self.indent.chars().count();
            }
            '\t' => {
                self.text.push_str(self.tab);
                self.chars += self.tab.chars().count();
            }
            c => {
                self.text.push(c);
                self.chars += 1;
            }
        }
    }

    /// A bare `$n`: the placeholder's text again (a mirror), or an empty stop.
    fn add_stop(&mut self, n: u32, start: usize) {
        if let Some(value) = self.values.get(&n).cloned() {
            // The value already contains its indentation; push it raw.
            self.text.push_str(&value);
            self.chars += value.chars().count();
        }
        self.stops.entry(n).or_default().push(start..self.chars);
    }

    fn push_str(&mut self, s: &str) {
        for c in s.chars() {
            self.push(c);
        }
    }
}

/// Parses until the end, or (inside a placeholder) until the closing `}`, which is consumed.
fn parse(chars: &[char], i: &mut usize, out: &mut Output, variable: &dyn Fn(&str) -> Option<String>, nested: bool) {
    while *i < chars.len() {
        let c = chars[*i];
        match c {
            '\\' if *i + 1 < chars.len() && matches!(chars[*i + 1], '$' | '}' | '\\') => {
                out.push(chars[*i + 1]);
                *i += 2;
            }
            '}' if nested => {
                *i += 1;
                return;
            }
            '$' => {
                *i += 1;
                dollar(chars, i, out, variable);
            }
            c => {
                out.push(c);
                *i += 1;
            }
        }
    }
}

fn dollar(chars: &[char], i: &mut usize, out: &mut Output, variable: &dyn Fn(&str) -> Option<String>) {
    let digits = |i: &mut usize| {
        let start = *i;
        while *i < chars.len() && chars[*i].is_ascii_digit() {
            *i += 1;
        }
        chars[start..*i].iter().collect::<String>().parse::<u32>().ok()
    };
    let name = |i: &mut usize| {
        let start = *i;
        while *i < chars.len() && (chars[*i].is_ascii_alphanumeric() || chars[*i] == '_') {
            *i += 1;
        }
        chars[start..*i].iter().collect::<String>()
    };
    let Some(&next) = chars.get(*i) else {
        out.push('$');
        return;
    };
    if next.is_ascii_digit() {
        let n = digits(i).unwrap_or(0);
        out.add_stop(n, out.chars);
    } else if next.is_ascii_alphabetic() || next == '_' {
        let var = name(i);
        match variable(&var) {
            Some(value) => out.push_str(&value),
            // Unknown variables are inserted as their name, like VS Code does.
            None => out.push_str(&var),
        }
    } else if next == '{' {
        *i += 1;
        let (start, byte_start) = (out.chars, out.text.len());
        if chars.get(*i).is_some_and(char::is_ascii_digit) {
            let n = digits(i).unwrap_or(0);
            match chars.get(*i) {
                Some(':') => {
                    *i += 1;
                    parse(chars, i, out, variable, true);
                }
                Some('|') => {
                    *i += 1;
                    let mut choice = String::new();
                    let mut first = true;
                    while *i < chars.len() && chars[*i] != '|' {
                        if chars[*i] == ',' {
                            first = false;
                        } else if first {
                            choice.push(chars[*i]);
                        }
                        *i += 1;
                    }
                    *i += 2; // `|}`
                    out.push_str(&choice);
                }
                _ => *i += 1, // `}`
            }
            let value = out.text[byte_start..].to_owned();
            let range = start..out.chars;
            // A later `${1:…}` for a stop that already has a placeholder is a mirror too.
            out.stops.entry(n).or_default().push(range);
            out.values.entry(n).or_insert(value);
        } else {
            let var = name(i);
            match (chars.get(*i), variable(&var)) {
                (Some(':'), value) => {
                    *i += 1;
                    match value {
                        Some(value) => {
                            out.push_str(&value);
                            // Skip the default.
                            let mut sink = Output { text: String::new(), chars: 0, stops: BTreeMap::new(), values: BTreeMap::new(), indent: "", tab: "" };
                            parse(chars, i, &mut sink, variable, true);
                        }
                        None => parse(chars, i, out, variable, true),
                    }
                }
                (_, value) => {
                    *i += 1; // `}`
                    out.push_str(&value.unwrap_or(var));
                }
            }
        }
    } else {
        out.push('$');
    }
}

/// Values for the snippet variables ByteForge knows, for a file `path`.
pub fn variable(name: &str, path: Option<&Path>) -> Option<String> {
    let stem = || path.and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().into_owned());
    match name {
        "TM_FILENAME_BASE" => stem(),
        "TM_FILENAME" => path.and_then(|p| p.file_name()).map(|s| s.to_string_lossy().into_owned()),
        "TM_DIRECTORY" => path.and_then(|p| p.parent()).map(|p| p.to_string_lossy().into_owned()),
        "TM_FILEPATH" => path.map(|p| p.to_string_lossy().into_owned()),
        "CURRENT_YEAR" => Some(year().to_string()),
        _ => None,
    }
}

fn year() -> i64 {
    let days = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() / 86_400).unwrap_or(0) as i64;
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    yoe + era * 400 + i64::from(mp >= 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> Option<String> {
        None
    }

    fn stop_texts(e: &Expansion) -> Vec<String> {
        e.stops.iter().map(|r| e.text.chars().skip(r[0].start).take(r[0].end - r[0].start).collect()).collect()
    }

    #[test]
    fn expands_tab_stops_and_defaults() {
        let e = expand("public ${1:int} ${2:MyProperty} { get; set; }$0", "", "    ", &none);
        assert_eq!(e.text, "public int MyProperty { get; set; }");
        assert_eq!(stop_texts(&e), ["int", "MyProperty", ""]);
        assert_eq!(e.stops[2].len(), 1);
        assert_eq!(e.stops[2][0], 35..35);

        // Mirrors repeat the placeholder; no $0 ends at the end.
        let e = expand("for (int ${1:i} = 0; $1 < ${2:n}; $1++)", "", "    ", &none);
        assert_eq!(e.text, "for (int i = 0; i < n; i++)");
        assert_eq!(stop_texts(&e), ["i", "n", ""]);
        assert_eq!(e.stops[0], [9..10, 16..17, 23..24]);
        assert_eq!(e.stops[2][0].start, e.text.chars().count());
    }

    #[test]
    fn indents_lines_and_tabs() {
        let e = expand("if (${1:true})\n{\n\t$0\n}", "    ", "  ", &none);
        assert_eq!(e.text, "if (true)\n    {\n      \n    }");
        let caret = e.stops[1][0].start;
        assert_eq!(e.text.chars().take(caret).collect::<String>(), "if (true)\n    {\n      ");
    }

    #[test]
    fn variables_choices_and_escapes() {
        let vars = |name: &str| (name == "TM_FILENAME_BASE").then(|| "Widget".to_owned());
        let e = expand("public ${1:$TM_FILENAME_BASE}($2) ${3|public,private|} \\$x ${UNKNOWN:fallback} $TM_FILENAME_BASE", "", "\t", &vars);
        assert_eq!(e.text, "public Widget() public $x fallback Widget");
        assert_eq!(stop_texts(&e), ["Widget", "", "public", ""]);
        assert_eq!(variable("TM_FILENAME_BASE", Some(Path::new("/x/Order.cs"))).as_deref(), Some("Order"));
        assert!(year() >= 2025);
    }

    #[test]
    fn parses_vscode_snippet_files() {
        let text = r#"{
            // A comment
            "Log": { "prefix": ["log", "lg"], "body": ["_logger.LogInformation(\"$1\");", "$0"], "description": "Log info" },
            "Bare": { "body": "x" }
        }"#;
        let snippets = parse_snippets(text).unwrap();
        assert_eq!(snippets.len(), 3);
        let log = snippets.iter().find(|s| s.prefix == "lg").unwrap();
        assert_eq!(log.body, "_logger.LogInformation(\"$1\");\n$0");
        assert_eq!(log.description, "Log info");
        assert!(snippets.iter().any(|s| s.prefix == "Bare" && s.description == "Bare"));

        let dir = std::env::temp_dir().join(format!("bf-snippets-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("csharp.json"), r#"{ "Mine": { "prefix": "prop", "body": "mine" } }"#).unwrap();
        let all = for_language(&dir, "csharp");
        assert_eq!(all.iter().filter(|s| s.prefix == "prop").map(|s| s.body.as_str()).collect::<Vec<_>>(), ["mine"]);
        assert!(all.iter().any(|s| s.prefix == "foreach"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
