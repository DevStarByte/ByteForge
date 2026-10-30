use super::*;
use std::time::Instant;

const SAMPLE: &str = r#"using System.Collections.Generic;

namespace Demo.App
{
    #region Types
    /// <summary>A sample.</summary>
    public class Widget : IDisposable
    {
        private const int MaxCount = 10;
        private readonly List<string> _items = new();
        public string Name { get; set; }
        public event EventHandler Changed;

        // plain comment
        public int Count(string prefix, int limit)
        {
            var text = "a\tb";
            var path = @"C:\temp";
            var list = Helpers.Create<int>(limit);
            if (prefix == null)
            {
                return 0;
            }
            Console.WriteLine(text.Length);
            return _items.Count + 42;
        }
    }

    public interface IShape { }
    public struct Point { }
    public enum Color { Red, Green }
    public record Person(string First);
    #endregion
}
"#;

/// The kind of the span exactly covering the `nth` occurrence of `needle`.
fn kind_of(spans: &[Span], text: &str, needle: &str, nth: usize) -> Option<&'static str> {
    let start = text
        .match_indices(needle)
        .nth(nth)
        .unwrap_or_else(|| panic!("{needle:?} #{nth} not in text"))
        .0;
    let end = start + needle.len();
    spans
        .iter()
        .find(|s| s.start == start && s.end == end)
        .map(|s| s.kind)
}

fn highlight_all(language: Language, text: &str) -> Vec<Span> {
    let mut h = Highlighter::new(language);
    h.parse(text);
    h.highlight(text, 0..text.len())
}

fn assert_well_formed(spans: &[Span], range: Range<usize>) {
    for pair in spans.windows(2) {
        assert!(pair[0].end <= pair[1].start, "overlap: {pair:?}");
    }
    for s in spans {
        assert!(
            s.start < s.end && s.start >= range.start && s.end <= range.end,
            "{s:?}"
        );
    }
}

#[test]
fn queries_compile() {
    for language in [Language::CSharp, Language::Json] {
        let h = Highlighter::new(language);
        assert_eq!(h.language(), language);
        // Every capture that is not a predicate helper must map to a kind.
        for (name, kind) in h.query.capture_names().iter().zip(&h.capture_kinds) {
            assert!(
                name.starts_with('_') || kind.is_some(),
                "unmapped capture @{name}"
            );
        }
    }
}

#[test]
fn highlighter_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<Highlighter>();
}

#[test]
fn language_from_path() {
    assert_eq!(
        Language::from_path(Path::new("a/B.cs")),
        Some(Language::CSharp)
    );
    assert_eq!(
        Language::from_path(Path::new("x.CSX")),
        Some(Language::CSharp)
    );
    assert_eq!(
        Language::from_path(Path::new("appsettings.json")),
        Some(Language::Json)
    );
    assert_eq!(
        Language::from_path(Path::new("tsconfig.jsonc")),
        Some(Language::Json)
    );
    assert_eq!(Language::from_path(Path::new("App.csproj")), None);
    assert_eq!(Language::from_path(Path::new("Makefile")), None);
    assert_eq!(Language::CSharp.name(), "C#");
    assert_eq!(Language::Json.name(), "JSON");
}

#[test]
fn csharp_kinds() {
    let spans = highlight_all(Language::CSharp, SAMPLE);
    assert_well_formed(&spans, 0..SAMPLE.len());
    let k = |needle: &str, nth: usize| kind_of(&spans, SAMPLE, needle, nth);

    assert_eq!(k("using", 0), Some("keyword"));
    assert_eq!(k("System", 0), Some("namespace name"));
    assert_eq!(k("Generic", 0), Some("namespace name"));
    assert_eq!(k("namespace", 0), Some("keyword"));
    assert_eq!(k("App", 0), Some("namespace name"));
    assert_eq!(k("#region", 0), Some("preprocessor keyword"));
    assert_eq!(k("#endregion", 0), Some("preprocessor keyword"));
    assert_eq!(
        k("/// <summary>A sample.</summary>", 0),
        Some("xml doc comment")
    );
    assert_eq!(k("// plain comment", 0), Some("comment"));
    assert_eq!(k("public", 0), Some("keyword"));
    assert_eq!(k("class", 0), Some("keyword"));
    assert_eq!(k("Widget", 0), Some("class name"));
    assert_eq!(k("IDisposable", 0), Some("interface name"));
    assert_eq!(k("const", 0), Some("keyword"));
    assert_eq!(k("int", 0), Some("keyword"));
    assert_eq!(k("MaxCount", 0), Some("constant name"));
    assert_eq!(k("10", 0), Some("number"));
    assert_eq!(k("List", 0), Some("class name"));
    assert_eq!(k("string", 0), Some("keyword"));
    assert_eq!(k("_items", 0), Some("field name"));
    assert_eq!(k("Name", 0), Some("property name"));
    assert_eq!(k("get", 1), Some("keyword"));
    assert_eq!(k("EventHandler", 0), Some("class name"));
    assert_eq!(k("Changed", 0), Some("event name"));
    assert_eq!(k("Count", 1), Some("method name"));
    assert_eq!(k("prefix", 0), Some("parameter name"));
    assert_eq!(k("limit", 0), Some("parameter name"));
    assert_eq!(k("var", 0), Some("keyword"));
    assert_eq!(k("text", 0), Some("local name"));
    assert_eq!(k("\"a", 0), Some("string"));
    assert_eq!(k("\\t", 0), Some("string - escape character"));
    assert_eq!(k("b\"", 0), Some("string"));
    assert_eq!(k(r#"@"C:\temp""#, 0), Some("string - verbatim"));
    assert_eq!(k("Create", 0), Some("method name"));
    assert_eq!(k("if", 0), Some("keyword - control"));
    assert_eq!(k("==", 0), Some("operator"));
    assert_eq!(k("null", 0), Some("keyword"));
    assert_eq!(k("return", 0), Some("keyword - control"));
    assert_eq!(k("WriteLine", 0), Some("method name"));
    assert_eq!(k("Length", 0), Some("property name"));
    assert_eq!(k("limit", 1), Some("identifier"));
    assert_eq!(k(";", 0), Some("punctuation"));
    assert_eq!(k("{", 0), Some("punctuation"));
    assert_eq!(k("IShape", 0), Some("interface name"));
    assert_eq!(k("Point", 0), Some("struct name"));
    assert_eq!(k("Color", 0), Some("enum name"));
    assert_eq!(k("Red", 0), Some("enum member name"));
    assert_eq!(k("Person", 0), Some("record class name"));
}

#[test]
fn json_kinds() {
    let text = "// c\n{ \"key\": \"value\", \"n\": -1.5e3, \"ok\": [true, null] }";
    let spans = highlight_all(Language::Json, text);
    assert_well_formed(&spans, 0..text.len());
    let k = |needle: &str| kind_of(&spans, text, needle, 0);
    assert_eq!(k("// c"), Some("comment"));
    assert_eq!(k("\"key\""), Some("json - property name"));
    assert_eq!(k("\"value\""), Some("json - string"));
    assert_eq!(k("-1.5e3"), Some("number"));
    assert_eq!(k("true"), Some("keyword"));
    assert_eq!(k("null"), Some("keyword"));
    assert_eq!(k("{"), Some("punctuation"));
    assert_eq!(k(":"), Some("punctuation"));
}

#[test]
fn highlight_clips_to_range() {
    let mut h = Highlighter::new(Language::CSharp);
    h.parse(SAMPLE);
    let start = SAMPLE.find("Widget").unwrap() + 2;
    let range = start..start + 40;
    let spans = h.highlight(SAMPLE, range.clone());
    assert_well_formed(&spans, range.clone());
    assert_eq!(
        spans[0],
        Span {
            start,
            end: start + 4,
            kind: "class name"
        }
    );
    // Out-of-bounds and empty ranges are tolerated.
    assert!(
        h.highlight(SAMPLE, SAMPLE.len()..SAMPLE.len() + 10)
            .is_empty()
    );
    assert!(
        Highlighter::new(Language::Json)
            .highlight("{}", 0..2)
            .is_empty()
    );
}

/// Computes the `Edit` for inserting `inserted` at byte `at` of `old`.
fn insertion(old: &str, at: usize, inserted: &str) -> Edit {
    let point = |s: &str, byte: usize| {
        let before = &s[..byte];
        let row = before.matches('\n').count();
        let col = byte - before.rfind('\n').map_or(0, |i| i + 1);
        (row, col)
    };
    let new = format!("{}{}{}", &old[..at], inserted, &old[at..]);
    Edit {
        start_byte: at,
        old_end_byte: at,
        new_end_byte: at + inserted.len(),
        start_point: point(old, at),
        old_end_point: point(old, at),
        new_end_point: point(&new, at + inserted.len()),
    }
}

#[test]
fn incremental_edit_matches_full_parse() {
    let mut h = Highlighter::new(Language::CSharp);
    h.parse(SAMPLE);
    let before = h.highlight(SAMPLE, 0..SAMPLE.len());
    let widget_before = kind_of(&before, SAMPLE, "Widget", 0);
    assert_eq!(widget_before, Some("class name"));

    // Insert a new field line before the class (shifts everything after it).
    let at = SAMPLE.find("    public class Widget").unwrap();
    let inserted = "    // inserted\n    public class Extra { int x; }\n";
    let edit = insertion(SAMPLE, at, inserted);
    let new_text = format!("{}{}{}", &SAMPLE[..at], inserted, &SAMPLE[at..]);
    h.edit(edit, &new_text);

    let after = h.highlight(&new_text, 0..new_text.len());
    assert_eq!(after, highlight_all(Language::CSharp, &new_text));
    assert_eq!(kind_of(&after, &new_text, "Extra", 0), Some("class name"));
    assert_eq!(
        kind_of(&after, &new_text, "// inserted", 0),
        Some("comment")
    );

    // The span for `Widget` moved by exactly the inserted length.
    let old_start = SAMPLE.find("Widget").unwrap();
    let shifted = after
        .iter()
        .find(|s| s.start == old_start + inserted.len())
        .unwrap();
    assert_eq!(shifted.end - shifted.start, "Widget".len());
    assert_eq!(shifted.kind, "class name");

    // Typing inside an identifier (mid-token edit) is also consistent.
    let at2 = new_text.find("prefix").unwrap() + 3;
    let edit2 = insertion(&new_text, at2, "Z");
    let text2 = format!("{}Z{}", &new_text[..at2], &new_text[at2..]);
    h.edit(edit2, &text2);
    let after2 = h.highlight(&text2, 0..text2.len());
    assert_eq!(after2, highlight_all(Language::CSharp, &text2));
    assert_eq!(
        kind_of(&after2, &text2, "preZfix", 0),
        Some("parameter name")
    );
}

fn big_csharp(lines: usize) -> String {
    const UNIT: &str = r#"    public class Worker{N} : IWorker
    {
        private readonly List<string> _items = new();
        /// <summary>Runs.</summary>
        public int Run(string input, int count)
        {
            var total = 0; // accumulate
            for (int i = 0; i < count; i++)
            {
                if (input.Length > i) { total += i * 2; }
            }
            Console.WriteLine($"done {total}\n");
            return total;
        }
    }
"#;
    let unit_lines = UNIT.lines().count();
    let mut s = String::from("namespace Perf\n{\n");
    for n in 0..lines / unit_lines {
        s.push_str(&UNIT.replace("{N}", &n.to_string()));
    }
    s.push_str("}\n");
    s
}

#[test]
fn perf_smoke() {
    let text = big_csharp(60_000);
    let mut h = Highlighter::new(Language::CSharp);
    let t = Instant::now();
    h.parse(&text);
    let parse = t.elapsed();

    let mid = text.len() / 2;
    let mid = (mid..).find(|&i| text.is_char_boundary(i)).unwrap();
    let range = mid..mid + 32 * 1024;
    let t = Instant::now();
    let spans = h.highlight(&text, range.clone());
    let highlight = t.elapsed();

    let at = mid + 100;
    let edit = insertion(&text, at, "x");
    let new_text = format!("{}x{}", &text[..at], &text[at..]);
    let t = Instant::now();
    h.edit(edit, &new_text);
    let reparse = t.elapsed();

    println!(
        "perf: {} bytes / {} lines, full parse {parse:?}, 32 KB highlight {highlight:?} ({} spans), incremental reparse {reparse:?}",
        text.len(),
        text.lines().count(),
        spans.len()
    );
    assert!(!spans.is_empty());
    assert_well_formed(&spans, range);
    if !cfg!(debug_assertions) {
        assert!(
            highlight.as_millis() < 50,
            "32 KB highlight took {highlight:?}"
        );
    }
}

#[test]
fn csharp_extra_constructs() {
    let text = r#"record struct Pt(int X);
class C<T> where T : class
{
    void M()
    {
        Func<int, int> f = x => x;
        var s = $"a{f(1)}b";
        var v = $@"c:\{s}";
        T d = default;
    start:
        goto start;
    }
}
"#;
    let spans = highlight_all(Language::CSharp, text);
    assert_well_formed(&spans, 0..text.len());
    let k = |needle: &str, nth: usize| kind_of(&spans, text, needle, nth);
    assert_eq!(k("Pt", 0), Some("struct name"));
    assert_eq!(k("T", 0), Some("type parameter name"));
    assert_eq!(k("T", 1), Some("type parameter name"));
    assert_eq!(k("Func", 0), Some("class name"));
    assert_eq!(k("x", 0), Some("parameter name"));
    assert_eq!(k("$\"a", 0), Some("string"));
    assert_eq!(k("f", 1), Some("method name"));
    assert_eq!(k("$@\"c:\\", 0), Some("string - verbatim"));
    assert_eq!(k("default", 0), Some("keyword"));
    assert_eq!(k("start", 0), Some("label name"));
    assert_eq!(k("goto", 0), Some("keyword - control"));
    assert_eq!(k("start", 1), Some("label name"));
}
