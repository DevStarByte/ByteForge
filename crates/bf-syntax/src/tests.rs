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
    for language in [Language::CSharp, Language::Json, Language::Razor, Language::Xml, Language::Yaml] {
        let h = Highlighter::new(language);
        assert_eq!(h.language(), language);
        // Every capture that is not a predicate helper must map to a kind.
        for (name, kind) in h.query.as_ref().expect("tree-sitter language").capture_names().iter().zip(&h.capture_kinds) {
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
    assert_eq!(Language::from_path(Path::new("Makefile")), None);
    assert_eq!(Language::CSharp.name(), "C#");
    assert_eq!(Language::Json.name(), "JSON");
    assert_eq!(Language::from_path(Path::new("Pages/Index.cshtml")), Some(Language::Razor));
    assert_eq!(Language::from_path(Path::new("Counter.razor")), Some(Language::Razor));
    assert_eq!(Language::Razor.name(), "Razor");
    for name in ["App.csproj", "Directory.Build.props", "Sdk.targets", "App.slnx", "web.config", "Strings.resx", "MainWindow.axaml"] {
        assert_eq!(Language::from_path(Path::new(name)), Some(Language::Xml), "{name}");
    }
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

const RAZOR_SAMPLE: &str = r#"@page "/counter"
@inject ILogger<Counter> Log
<h1 class="title">Counter @count</h1>
<button @onclick="Increment" disabled>Click</button>
@if (count > 3) { <p>Big @(count * 2)</p> }
@* razor comment *@
<!-- html comment -->
<script>var o = { a: 1 };</script>
@code {
    private int count = 0;
    private void Increment() { count++; }
}
"#;

#[test]
fn razor_kinds() {
    let text = RAZOR_SAMPLE;
    let spans = highlight_all(Language::Razor, text);
    assert_well_formed(&spans, 0..text.len());
    let k = |needle, nth| kind_of(&spans, text, needle, nth);
    // Transitions, directives and control flow.
    assert_eq!(k("@", 0), Some("razor transition"));
    assert_eq!(k("page", 0), Some("razor directive"));
    assert_eq!(k("inject", 0), Some("razor directive"));
    assert_eq!(k("if", 0), Some("keyword - control"));
    assert_eq!(k("code", 0), Some("razor directive"));
    assert_eq!(k("\"/counter\"", 0), Some("string"));
    // Markup.
    assert_eq!(k("h1", 0), Some("html element name"));
    assert_eq!(k("h1", 1), Some("html element name"));
    assert_eq!(k("class", 0), Some("html attribute name"));
    assert_eq!(k("\"title\"", 0), Some("html attribute value"));
    assert_eq!(k("disabled", 0), Some("html attribute name"));
    assert_eq!(k("</", 0), Some("html tag delimiter"));
    assert_eq!(k("onclick", 0), Some("razor directive"));
    assert_eq!(k("<!-- html comment -->", 0), Some("comment"));
    assert_eq!(k("@* razor comment *@", 0), Some("comment"));
    // Script bodies are not markup.
    let a = text.find("a: 1").unwrap();
    assert!(!spans.iter().any(|s| s.start <= a && a < s.end && s.kind.starts_with("html")));
    // Embedded C#.
    assert_eq!(k("int", 0), Some("keyword"));
    assert_eq!(k("Increment", 1), Some("method name"));
    assert_eq!(k("ILogger", 0), Some("interface name"));
}

#[test]
fn razor_window_matches_full_highlight() {
    let text = RAZOR_SAMPLE;
    let mut h = Highlighter::new(Language::Razor);
    h.parse(text);
    let full = h.highlight(text, 0..text.len());
    let (a, b) = (text.find("<button").unwrap() + 3, text.find("@code").unwrap());
    let window = h.highlight(text, a..b);
    let clipped: Vec<Span> = full
        .iter()
        .filter(|s| s.end > a && s.start < b)
        .map(|s| Span { start: s.start.max(a), end: s.end.min(b), kind: s.kind })
        .collect();
    assert_eq!(window, clipped);
}

const CSPROJ_SAMPLE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<Project Sdk="Microsoft.NET.Sdk.Web">
  <!-- Build settings -->
  <PropertyGroup Condition="'$(Configuration)' == 'Debug'">
    <TargetFramework>net10.0</TargetFramework>
    <OutputPath>$(BaseDir)bin/</OutputPath>
    <NoWarn>$(NoWarn);CS1591 &amp; more</NoWarn>
  </PropertyGroup>
  <ItemGroup>
    <PackageReference Include="Serilog" Version="4.0.0" />
  </ItemGroup>
</Project>
"#;

#[test]
fn xml_kinds() {
    let text = CSPROJ_SAMPLE;
    let spans = highlight_all(Language::Xml, text);
    assert_well_formed(&spans, 0..text.len());
    let k = |needle, nth| kind_of(&spans, text, needle, nth);
    assert_eq!(k("xml", 0), Some("keyword"));
    assert_eq!(k("Project", 0), Some("html element name"));
    assert_eq!(k("Project", 1), Some("html element name"));
    assert_eq!(k("Sdk", 0), Some("html attribute name"));
    assert_eq!(k("\"Microsoft.NET.Sdk.Web\"", 0), Some("html attribute value"));
    assert_eq!(k("<!-- Build settings -->", 0), Some("comment"));
    assert_eq!(k("PackageReference", 0), Some("html element name"));
    assert_eq!(k("/>", 0), Some("html tag delimiter"));
    assert_eq!(k("&amp;", 0), Some("string - escape character"));
    // MSBuild references in attribute values and in text.
    assert_eq!(k("$(Configuration)", 0), Some("xml - msbuild reference"));
    assert_eq!(k("$(BaseDir)", 0), Some("xml - msbuild reference"));
    assert_eq!(k("$(NoWarn)", 0), Some("xml - msbuild reference"));
}

#[test]
fn module_languages_detect_files_and_highlight_with_regexes() {
    use crate::module_lang::{ModuleLanguage, RegexRule, set_module_languages};
    let _guard = MODULE_LANGUAGES.lock().unwrap_or_else(|e| e.into_inner());
    set_module_languages(vec![ModuleLanguage {
        id: "dockerfile",
        name: "Dockerfile",
        line_comment: Some("#"),
        matches: Box::new(|name: &str| name.eq_ignore_ascii_case("Dockerfile")),
        rules: vec![
            RegexRule::new(r"^\s*#.*$", 0, "comment").unwrap(),
            RegexRule::new(r"(?i)^\s*(FROM|RUN)\b", 1, "keyword").unwrap(),
            RegexRule::new(r#""[^"]*""#, 0, "string").unwrap(),
            RegexRule::new(r"\b(AS)\b", 1, "keyword").unwrap(),
        ],
        detect: Vec::new(),
    }]);
    let lang = Language::from_path(Path::new("src/Api/Dockerfile")).expect("module language");
    assert_eq!((lang.id(), lang.name(), lang.line_comment()), ("dockerfile", "Dockerfile", Some("#")));
    assert_eq!(Language::from_id("dockerfile"), Some(lang));
    assert_eq!(Language::from_path(Path::new("compose.yaml")), Some(Language::Yaml));

    let text = "# base\nFROM sdk AS build\nRUN echo \"# not a comment\"\n";
    let spans = highlight_all(lang, text);
    assert_well_formed(&spans, 0..text.len());
    let k = |needle: &str| kind_of(&spans, text, needle, 0);
    assert_eq!(k("# base"), Some("comment"));
    assert_eq!(k("FROM"), Some("keyword"));
    assert_eq!(k("AS"), Some("keyword"));
    assert_eq!(k("\"# not a comment\""), Some("string"));
    // A window in the middle of a line still gets that line's spans, clipped.
    let mid = text.find("sdk").unwrap();
    let h = Highlighter::new(lang);
    assert!(h.highlight(text, mid..mid + 6).iter().any(|s| s.kind == "keyword" && s.start >= mid));

    set_module_languages(Vec::new());
    assert_eq!(Language::from_path(Path::new("Dockerfile")), None);
    assert!(Highlighter::new(lang).highlight(text, 0..text.len()).is_empty());
}

#[test]
fn yaml_kinds() {
    let text = "# compose\nservices:\n  api:\n    image: \"api:dev\"\n    ports:\n      - 8080:8080\n    restart: true\n    replicas: 2\n";
    let spans = highlight_all(Language::Yaml, text);
    assert_well_formed(&spans, 0..text.len());
    let k = |needle: &str| kind_of(&spans, text, needle, 0);
    assert_eq!(k("# compose"), Some("comment"));
    assert_eq!(k("services"), Some("json - property name"));
    assert_eq!(k("api"), Some("json - property name"));
    assert_eq!(k("\"api:dev\""), Some("string"));
    assert_eq!(k("true"), Some("keyword"));
    assert_eq!(k("2"), Some("number"));
}

#[test]
fn builtin_languages_from_path() {
    let id = |name: &str| Language::from_path(Path::new(name)).map(Language::id);
    for name in [".gitignore", "src/.dockerignore", ".npmignore", ".prettierignore", "VisualStudio.gitignore"] {
        assert_eq!(id(name), Some("ignore"), "{name}");
    }
    assert_eq!(id(".gitattributes"), Some("gitattributes"));
    for name in [".editorconfig", ".gitconfig", ".gitmodules", "setup.cfg", "php.ini", "app.desktop", "sshd.service"] {
        assert_eq!(id(name), Some("ini"), "{name}");
    }
    for name in [".env", ".env.local", ".env.production", "prod.env"] {
        assert_eq!(id(name), Some("dotenv"), "{name}");
    }
    // Not taken over: XML configs, files that merely contain "ignore".
    assert_eq!(Language::from_path(Path::new("web.config")), Some(Language::Xml));
    assert_eq!(Language::from_path(Path::new("ignore.txt")), None);
    assert_eq!(Language::from_path(Path::new(".gitignore")).and_then(Language::line_comment), Some("#"));
    assert_eq!(Language::from_id("gitignore").map(Language::id), Some("ignore"));
    assert_eq!(Language::from_id("env").map(Language::id), Some("dotenv"));
    assert_eq!(Language::from_id("editorconfig").map(Language::id), Some("ini"));
}

#[test]
fn builtin_language_kinds() {
    let lang = |name: &str| Language::from_path(Path::new(name)).unwrap();

    let text = "# build output\nbin/\n!keep.txt\n**/*.user\n[Dd]ebug/\n\\#literal\n";
    let spans = highlight_all(lang(".gitignore"), text);
    assert_well_formed(&spans, 0..text.len());
    assert_eq!(kind_of(&spans, text, "# build output", 0), Some("comment"));
    assert_eq!(kind_of(&spans, text, "!", 0), Some("operator"));
    assert_eq!(kind_of(&spans, text, "**", 0), Some("operator"));
    assert_eq!(kind_of(&spans, text, "[Dd]", 0), Some("string"));
    assert_eq!(kind_of(&spans, text, "\\#", 0), Some("string - escape character"));
    assert_eq!(kind_of(&spans, text, "keep.txt", 0), None);

    let text = "* text=auto\n*.png binary -diff\n[attr]bin -text\n";
    let spans = highlight_all(lang(".gitattributes"), text);
    assert_well_formed(&spans, 0..text.len());
    assert_eq!(kind_of(&spans, text, "*.png", 0), Some("string"));
    assert_eq!(kind_of(&spans, text, "binary", 0), Some("property name"));
    assert_eq!(kind_of(&spans, text, "-", 0), Some("operator"));
    assert_eq!(kind_of(&spans, text, "auto", 0), Some("constant name"));
    assert_eq!(kind_of(&spans, text, "[attr]", 0), Some("keyword"));

    let text = "root = true\n[*.cs]\nindent_size = 4 ; four\ncharset=utf-8\n";
    let spans = highlight_all(lang(".editorconfig"), text);
    assert_well_formed(&spans, 0..text.len());
    assert_eq!(kind_of(&spans, text, "root", 0), Some("property name"));
    assert_eq!(kind_of(&spans, text, "true", 0), Some("keyword"));
    assert_eq!(kind_of(&spans, text, "*.cs", 0), Some("class name"));
    assert_eq!(kind_of(&spans, text, "[", 0), Some("punctuation"));
    assert_eq!(kind_of(&spans, text, "; four", 0), Some("comment"));
    assert_eq!(kind_of(&spans, text, "=", 2), Some("operator"));

    let text = "# secrets\nexport API_KEY=abc # note\nURL=\"http://${HOST}:80\"\nRAW='$NOPE'\n";
    let spans = highlight_all(lang(".env.local"), text);
    assert_well_formed(&spans, 0..text.len());
    assert_eq!(kind_of(&spans, text, "export", 0), Some("keyword"));
    assert_eq!(kind_of(&spans, text, "API_KEY", 0), Some("property name"));
    assert_eq!(kind_of(&spans, text, "# note", 0), Some("comment"));
    assert_eq!(kind_of(&spans, text, "${HOST}", 0), Some("local name"));
    assert_eq!(kind_of(&spans, text, "'$NOPE'", 0), Some("string"));
}

/// Tests that register module languages (a global list) take turns.
static MODULE_LANGUAGES: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn detects_the_language_from_the_text() {
    let _guard = MODULE_LANGUAGES.lock().unwrap_or_else(|e| e.into_inner());
    let detect = |text: &str| Language::detect(text).map(Language::id);
    assert_eq!(detect(""), None);
    assert_eq!(detect("hello world\nsome notes"), None);
    assert_eq!(detect("using System;\n\nConsole.WriteLine(\"hi\");"), Some("csharp"));
    assert_eq!(detect("namespace Demo;\n\npublic class Foo\n{\n}"), Some("csharp"));
    assert_eq!(detect("var x = 1;\nreturn x;"), Some("csharp"));
    assert_eq!(detect("{\n  \"name\": \"demo\",\n  \"version\": 1\n}"), Some("json"));
    assert_eq!(detect("[1, 2, 3]"), Some("json"));
    assert_eq!(detect("[{\"a\": 1}]"), Some("json"));
    assert_eq!(detect("<?xml version=\"1.0\"?>\n<root />"), Some("xml"));
    assert_eq!(detect("<Project Sdk=\"Microsoft.NET.Sdk\">\n</Project>"), Some("xml"));
    assert_eq!(detect("@page \"/counter\"\n<h1>Counter</h1>\n@code {\n}"), Some("razor"));
    assert_eq!(detect("name: demo\nservices:\n  web:\n    image: nginx\n    ports:\n      - \"80:80\""), Some("yaml"));
    assert_eq!(detect("---\nfoo"), Some("yaml"));
    assert_eq!(detect("[core]\nautocrlf = true\neditor=vim"), Some("ini"));
    assert_eq!(detect("DATABASE_URL=postgres://x\nPORT=8080"), Some("dotenv"));
}

#[test]
fn module_languages_detect_their_text() {
    use crate::module_lang::{ModuleLanguage, detect_pattern, set_module_languages};
    let _guard = MODULE_LANGUAGES.lock().unwrap_or_else(|e| e.into_inner());
    set_module_languages(vec![ModuleLanguage {
        id: "sql",
        name: "SQL",
        line_comment: Some("--"),
        matches: Box::new(|name: &str| name.ends_with(".sql")),
        rules: Vec::new(),
        detect: [r"(?i)^\s*select\b.*\bfrom\b", r"(?i)^\s*(from|where|order\s+by)\b.*$", r"(?i)^\s*(insert\s+into|delete\s+from)\b"]
            .iter()
            .filter_map(|p| detect_pattern(p))
            .collect(),
    }]);
    let detect = |text: &str| Language::detect(text).map(Language::id);
    // One match is enough when nothing built-in fits; two win over the built-in guesses.
    assert_eq!(detect("SELECT * FROM users;"), Some("sql"));
    assert_eq!(detect("select id, name\nfrom users\nwhere id = 1;"), Some("sql"));
    assert_eq!(detect("SELECT * FROM users;\nDELETE FROM users;\nINSERT INTO users VALUES (1);"), Some("sql"));
    // C# with a LINQ query stays C#.
    assert_eq!(detect("using System.Linq;\nvar adults = from p in people\n    where p.Age > 17\n    select p;"), Some("csharp"));
    set_module_languages(Vec::new());
}

/// The kind of the span covering the first byte of `needle` (the `nth` occurrence).
fn kind_at(spans: &[Span], text: &str, needle: &str, nth: usize) -> Option<&'static str> {
    let at = text.match_indices(needle).nth(nth).unwrap_or_else(|| panic!("{needle:?} #{nth} not in text")).0;
    spans.iter().find(|s| s.start <= at && at < s.end).map(|s| s.kind)
}

#[test]
fn web_and_script_languages_from_path() {
    let cases = [
        ("app.js", Language::JavaScript),
        ("vite.config.mjs", Language::JavaScript),
        ("Button.jsx", Language::JavaScript),
        ("main.ts", Language::TypeScript),
        ("App.tsx", Language::Tsx),
        ("wwwroot/css/site.css", Language::Css),
        ("_variables.scss", Language::Scss),
        ("wwwroot/index.html", Language::Html),
        ("Migrations/001.sql", Language::Sql),
        ("README.md", Language::Markdown),
        ("build.ps1", Language::PowerShell),
        ("deploy.sh", Language::Shell),
        ("PKGBUILD", Language::Shell),
        (".bashrc", Language::Shell),
        (".zshrc", Language::Shell),
    ];
    for (name, language) in cases {
        assert_eq!(Language::from_path(Path::new(name)), Some(language), "{name}");
        assert_eq!(Language::from_id(language.id()), Some(language), "{name}: id round trip");
        // Every query compiles, with its injections.
        let mut h = Highlighter::new(language);
        h.parse("x");
    }
    assert_eq!(Language::from_id("bash"), Some(Language::Shell));
    assert_eq!(Language::from_id("ts"), Some(Language::TypeScript));
    assert_eq!(Language::Sql.line_comment(), Some("--"));
}

#[test]
fn detects_scripts_by_their_first_line() {
    assert_eq!(Language::detect("#!/bin/bash\necho hi\n"), Some(Language::Shell));
    assert_eq!(Language::detect("#!/usr/bin/env sh\nset -e\n"), Some(Language::Shell));
    assert_eq!(Language::detect("#!/usr/bin/env node\nconsole.log(1)\n"), Some(Language::JavaScript));
    assert_eq!(Language::detect("#!/usr/bin/env pwsh\nWrite-Host hi\n"), Some(Language::PowerShell));
    assert_eq!(Language::detect("<!DOCTYPE html>\n<html></html>\n"), Some(Language::Html));
    assert_eq!(Language::detect("<?xml version=\"1.0\"?><a/>"), Some(Language::Xml));
}

#[test]
fn javascript_and_typescript_kinds() {
    let js = "// note\nimport x from \"y\";\nfunction add(a, b) { return a + b; }\nconst o = { name: 'n', n: 42 };\no.call(true);\n";
    let spans = highlight_all(Language::JavaScript, js);
    assert_well_formed(&spans, 0..js.len());
    let k = |needle: &str| kind_at(&spans, js, needle, 0);
    assert_eq!(k("// note"), Some("comment"));
    assert_eq!(k("import"), Some("keyword"));
    assert_eq!(k("\"y\""), Some("string"));
    assert_eq!(k("add"), Some("method name"));
    assert_eq!(k("return"), Some("keyword"));
    assert_eq!(k("42"), Some("number"));
    assert_eq!(k("call"), Some("method name"));
    assert_eq!(k("true"), Some("keyword"));

    let ts = "interface User { id: number; name?: string }\nexport class Repo<T> { private items: T[] = []; }\n";
    let spans = highlight_all(Language::TypeScript, ts);
    assert_well_formed(&spans, 0..ts.len());
    let k = |needle: &str| kind_at(&spans, ts, needle, 0);
    assert_eq!(k("interface"), Some("keyword"));
    assert_eq!(k("User"), Some("class name"));
    assert_eq!(k("number"), Some("keyword"));
    assert_eq!(k("private"), Some("keyword"));
    assert_eq!(k("Repo"), Some("class name"));

    let tsx = "const App = () => <div className=\"x\">{count}</div>;\n";
    let spans = highlight_all(Language::Tsx, tsx);
    assert_eq!(kind_at(&spans, tsx, "div", 0), Some("html element name"));
    assert_eq!(kind_at(&spans, tsx, "className", 0), Some("html attribute name"));
}

#[test]
fn css_scss_and_html_kinds() {
    let css = "/* c */\n.card > a:hover { color: #fff; margin: 4px !important; }\n@media (min-width: 600px) { body { display: grid; } }\n";
    let spans = highlight_all(Language::Css, css);
    assert_well_formed(&spans, 0..css.len());
    let k = |needle: &str| kind_at(&spans, css, needle, 0);
    assert_eq!(k("/* c */"), Some("comment"));
    assert_eq!(k("color"), Some("property name"));
    assert_eq!(k("4px"), Some("number"));
    assert_eq!(k("@media"), Some("keyword"));

    let scss = "$gap: 8px;\n@mixin pad { padding: $gap; }\n.a { @include pad; &:hover { color: red; } }\n";
    let spans = highlight_all(Language::Scss, scss);
    assert_well_formed(&spans, 0..scss.len());
    assert_eq!(kind_at(&spans, scss, "@mixin", 0), Some("keyword"));
    assert_eq!(kind_at(&spans, scss, "padding", 0), Some("property name"));

    let html = "<!-- c -->\n<div class=\"x\">Hi</div>\n<script>const n = 1;</script>\n<style>p { color: red; }</style>\n";
    let spans = highlight_all(Language::Html, html);
    assert_well_formed(&spans, 0..html.len());
    let k = |needle: &str| kind_at(&spans, html, needle, 0);
    assert_eq!(k("<!-- c -->"), Some("comment"));
    assert_eq!(k("div"), Some("html element name"));
    assert_eq!(k("class"), Some("html attribute name"));
    assert_eq!(k("\"x\""), Some("html attribute value"));
    assert_eq!(k("<div"), Some("html tag delimiter"));
    // The injected grammars: JavaScript in <script>, CSS in <style>.
    assert_eq!(k("const"), Some("keyword"));
    assert_eq!(k("1;"), Some("number"));
    assert_eq!(k("color"), Some("property name"));
}

#[test]
fn sql_powershell_and_shell_kinds() {
    let sql = "-- users\nSELECT id, name FROM users WHERE id = 42 AND name LIKE 'a%';\n";
    let spans = highlight_all(Language::Sql, sql);
    assert_well_formed(&spans, 0..sql.len());
    let k = |needle: &str| kind_at(&spans, sql, needle, 0);
    assert_eq!(k("-- users"), Some("comment"));
    assert_eq!(k("SELECT"), Some("keyword"));
    assert_eq!(k("FROM"), Some("keyword"));
    assert_eq!(k("42"), Some("number"));
    assert_eq!(k("'a%'"), Some("string"));

    let ps = "# build\nparam([string]$Configuration = \"Release\")\nif ($Configuration -eq 'Debug') { Write-Host \"debug\" }\n";
    let spans = highlight_all(Language::PowerShell, ps);
    assert_well_formed(&spans, 0..ps.len());
    let k = |needle: &str| kind_at(&spans, ps, needle, 0);
    assert_eq!(k("# build"), Some("comment"));
    assert_eq!(k("if"), Some("keyword"));
    assert_eq!(k("\"Release\""), Some("string"));
    assert_eq!(k("Write-Host"), Some("method name"));

    let sh = "#!/bin/bash\n# deploy\nset -e\nfor f in *.txt; do echo \"$f\"; done\nNAME=x\n";
    let spans = highlight_all(Language::Shell, sh);
    assert_well_formed(&spans, 0..sh.len());
    let k = |needle: &str| kind_at(&spans, sh, needle, 0);
    assert_eq!(k("# deploy"), Some("comment"));
    assert_eq!(k("for"), Some("keyword"));
    assert_eq!(k("echo"), Some("method name"));
    assert_eq!(k("\"$f\""), Some("string"));
}

#[test]
fn markdown_blocks_and_inline() {
    let md = "# Title\n\nSome **bold**, *italic* and `code` with a [link](https://x.y).\n\n```cs\nvar x = 1;\n```\n\n- item\n";
    let spans = highlight_all(Language::Markdown, md);
    assert_well_formed(&spans, 0..md.len());
    let k = |needle: &str| kind_at(&spans, md, needle, 0);
    assert_eq!(k("Title"), Some("markup heading"));
    assert_eq!(k("bold"), Some("markup bold"));
    assert_eq!(k("italic"), Some("markup italic"));
    assert_eq!(kind_at(&spans, md, "code", 0), Some("markup code"));
    assert_eq!(k("https://x.y"), Some("markup link"));
    assert_eq!(k("var x"), Some("markup code"));
    assert_eq!(k("Some"), None, "plain text stays plain");
}

#[test]
fn injections_follow_edits() {
    let before = "<p>x</p>\n";
    let mut h = Highlighter::new(Language::Html);
    h.parse(before);
    let after = "<p>x</p>\n<script>let a = 2;</script>\n";
    h.edit(insertion(before, before.len(), &after[before.len()..]), after);
    let spans = h.highlight(after, 0..after.len());
    assert_eq!(kind_at(&spans, after, "let", 0), Some("keyword"), "the new <script> is highlighted as JavaScript");
}
