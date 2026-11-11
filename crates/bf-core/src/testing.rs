//! .NET tests: discovery from source (the test attributes of xUnit, NUnit and MSTest, so it's instant and
//! knows where each test is), `dotnet test` filters, and results from TRX files.
//!
//! Names follow VSTest: `Namespace.Class.Method`, nested classes joined with `+`. A filter
//! `FullyQualifiedName=Namespace.Class.Method` also matches every case of a theory / `[TestCase]`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Attributes that mark a test method (with or without the `Attribute` suffix or a namespace).
const TEST_ATTRIBUTES: &[&str] =
    &["Fact", "Theory", "SkippableFact", "SkippableTheory", "Test", "TestCase", "TestCaseSource", "TestMethod", "DataTestMethod"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestCase {
    /// Method name.
    pub name: String,
    /// `Namespace.Class` (nested classes: `Namespace.Outer+Inner`).
    pub class: String,
    pub file: PathBuf,
    /// 0-based line of the method name.
    pub line: usize,
}

impl TestCase {
    pub fn full_name(&self) -> String {
        format!("{}.{}", self.class, self.name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestClass {
    /// `Namespace.Class` as in [`TestCase::class`].
    pub full_name: String,
    pub file: PathBuf,
    pub line: usize,
    pub tests: Vec<TestCase>,
}

impl TestClass {
    /// The class name without namespace (`Outer+Inner` for nested classes).
    pub fn short_name(&self) -> &str {
        let head = self.full_name.split('+').next().unwrap_or(&self.full_name);
        let ns_len = head.rfind('.').map_or(0, |i| i + 1);
        &self.full_name[ns_len..]
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestProject {
    /// The `.csproj`.
    pub path: PathBuf,
    pub name: String,
    pub classes: Vec<TestClass>,
}

impl TestProject {
    pub fn test_count(&self) -> usize {
        self.classes.iter().map(|c| c.tests.len()).sum()
    }
}

/// Whether a `.csproj` is a test project (the test SDK, a test framework, or `IsTestProject`).
pub fn is_test_project_text(csproj: &str) -> bool {
    let lower = csproj.to_lowercase();
    ["microsoft.net.test.sdk", "<istestproject>true", "xunit", "nunit", "mstest", "tunit"].iter().any(|m| lower.contains(m))
}

/// The test projects below `root` with their tests, found by scanning their `.cs` files.
pub fn discover(root: &Path) -> Vec<TestProject> {
    let projects = crate::dotnet::projects(root);
    let mut result = Vec::new();
    for project in &projects {
        if !std::fs::read_to_string(project).is_ok_and(|t| is_test_project_text(&t)) {
            continue;
        }
        let Some(dir) = project.parent() else { continue };
        // Folders of other projects nested inside this one belong to them.
        let others: Vec<&Path> = projects.iter().filter(|p| *p != project).filter_map(|p| p.parent()).filter(|d| *d != dir && d.starts_with(dir)).collect();
        let mut files = Vec::new();
        collect_sources(dir, &others, 0, &mut files);
        files.sort();
        let mut classes: Vec<TestClass> = Vec::new();
        for file in files {
            let Ok(text) = std::fs::read_to_string(&file) else { continue };
            for class in scan_source(&text, &file) {
                // Partial classes split over files: merge.
                match classes.iter_mut().find(|c| c.full_name == class.full_name) {
                    Some(existing) => existing.tests.extend(class.tests),
                    None => classes.push(class),
                }
            }
        }
        if !classes.is_empty() {
            let name = project.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            result.push(TestProject { path: project.clone(), name, classes });
        }
    }
    result
}

pub(crate) fn collect_sources(dir: &Path, skip: &[&Path], depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 8 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if !matches!(name.as_str(), "bin" | "obj" | ".git" | "node_modules" | "TestResults") && !skip.contains(&path.as_path()) {
                collect_sources(&path, skip, depth + 1, out);
            }
        } else if name.ends_with(".cs") {
            out.push(path);
        }
    }
}

/// The classes in a C# file that contain test methods.
pub fn scan_source(text: &str, file: &Path) -> Vec<TestClass> {
    let mut classes: Vec<TestClass> = Vec::new();
    let mut namespace = String::new();
    // Block-scoped namespaces: (name, depth inside its braces).
    let mut ns_stack: Vec<(String, usize)> = Vec::new();
    let mut pending_ns: Option<String> = None;
    // Open classes: (name, line, depth inside its braces).
    let mut class_stack: Vec<(String, usize, usize)> = Vec::new();
    let mut pending_class: Option<(String, usize)> = None;
    let mut test_attribute = false;
    let mut depth = 0usize;
    let mut in_block_comment = false;

    for (line_ix, raw) in text.lines().enumerate() {
        let code = strip_code(raw, &mut in_block_comment);
        let trimmed = code.trim();

        if let Some(rest) = trimmed.strip_prefix("namespace ") {
            let name = rest.trim_end_matches(['{', ';', ' ']).trim().to_owned();
            if trimmed.ends_with(';') {
                namespace = name;
            } else {
                pending_ns = Some(name);
            }
        }
        let (attrs, trimmed) = split_attributes(trimmed);
        if attributes(attrs).iter().any(|a| TEST_ATTRIBUTES.contains(&a.as_str())) {
            test_attribute = true;
        }
        if trimmed.is_empty() {
            // Only attributes on this line.
        } else if let Some(name) = class_name(trimmed) {
            pending_class = Some((name, line_ix));
            test_attribute = false;
        } else if test_attribute
            && let Some(name) = method_name(trimmed)
            && let Some((_, _, _)) = class_stack.last()
        {
            let class = class_full_name(&namespace, &ns_stack, &class_stack);
            let case = TestCase { name, class: class.clone(), file: file.to_path_buf(), line: line_ix };
            match classes.iter_mut().find(|c| c.full_name == class) {
                Some(c) => c.tests.push(case),
                None => {
                    let (_, class_line, _) = class_stack.last().cloned().unwrap_or_default();
                    classes.push(TestClass { full_name: class, file: file.to_path_buf(), line: class_line, tests: vec![case] });
                }
            }
            test_attribute = false;
        }

        for ch in code.chars() {
            match ch {
                '{' => {
                    depth += 1;
                    if let Some(name) = pending_ns.take() {
                        ns_stack.push((name, depth));
                    } else if let Some((name, line)) = pending_class.take() {
                        class_stack.push((name, line, depth));
                    }
                }
                '}' => {
                    if class_stack.last().is_some_and(|c| c.2 == depth) {
                        class_stack.pop();
                    }
                    if ns_stack.last().is_some_and(|n| n.1 == depth) {
                        ns_stack.pop();
                    }
                    depth = depth.saturating_sub(1);
                }
                // `class Foo;` (no body) or a record declaration without braces.
                ';' if pending_class.is_some() && !code.contains('{') => pending_class = None,
                _ => {}
            }
        }
    }
    classes
}

fn class_full_name(file_ns: &str, ns_stack: &[(String, usize)], class_stack: &[(String, usize, usize)]) -> String {
    let mut ns: Vec<&str> = Vec::new();
    if !file_ns.is_empty() {
        ns.push(file_ns);
    }
    ns.extend(ns_stack.iter().map(|(n, _)| n.as_str()));
    let classes: Vec<&str> = class_stack.iter().map(|(n, _, _)| n.as_str()).collect();
    let class = classes.join("+");
    if ns.is_empty() { class } else { format!("{}.{class}", ns.join(".")) }
}

/// The line without comments and string contents (so braces in them don't count).
fn strip_code(line: &str, in_block_comment: &mut bool) -> String {
    let mut out = String::with_capacity(line.len());
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    let mut in_string: Option<char> = None;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if *in_block_comment {
            if c == '*' && next == Some('/') {
                *in_block_comment = false;
                i += 1;
            }
        } else if let Some(quote) = in_string {
            if c == '\\' {
                i += 1;
            } else if c == quote {
                in_string = None;
                out.push(c);
            }
        } else if c == '/' && next == Some('/') {
            break;
        } else if c == '/' && next == Some('*') {
            *in_block_comment = true;
            i += 1;
        } else if c == '"' || c == '\'' {
            in_string = Some(c);
            out.push(c);
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

/// Splits `[A] [B(…)] rest` into the attribute sections and the rest.
fn split_attributes(line: &str) -> (&str, &str) {
    let mut depth = 0i32;
    let mut end = 0;
    for (i, c) in line.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    end = i + 1;
                }
            }
            c if depth == 0 && !c.is_whitespace() => break,
            _ => {}
        }
    }
    (&line[..end], line[end..].trim_start())
}

/// Attribute names in `[A, B(…)]` (several brackets per line allowed), without `Attribute` or namespace.
fn attributes(line: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut depth = 0;
    let mut current = String::new();
    let mut inside = false;
    for c in line.chars() {
        match c {
            '[' if depth == 0 => {
                inside = true;
                current.clear();
            }
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' | ']' if depth == 0 && inside => {
                let name = current.trim();
                let name = name.rsplit('.').next().unwrap_or(name);
                let name = name.strip_suffix("Attribute").unwrap_or(name);
                let name = name.trim_start_matches("global::");
                if !name.is_empty() {
                    names.push(name.to_owned());
                }
                current.clear();
                if c == ']' {
                    inside = false;
                }
            }
            _ if depth == 0 && inside => current.push(c),
            _ => {}
        }
    }
    names
}

/// `public sealed class Foo<T> : Bar` → `Foo`.
fn class_name(line: &str) -> Option<String> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let i = words.iter().position(|w| *w == "class" || *w == "record")?;
    // `record struct` isn't a class; `class` must follow modifiers only.
    let modifiers = ["public", "internal", "private", "protected", "static", "sealed", "abstract", "partial", "file", "unsafe", "new"];
    if !words[..i].iter().all(|w| modifiers.contains(w)) {
        return None;
    }
    let name = words.get(i + 1)?;
    let name: String = name.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
    (!name.is_empty() && name != "struct").then_some(name)
}

/// `public async Task Adds_numbers(int a)` → `Adds_numbers`.
fn method_name(line: &str) -> Option<String> {
    let paren = line.find('(')?;
    let head = line[..paren].trim_end();
    let head = match head.strip_suffix('>') {
        // Generic method: drop the type parameters.
        Some(h) => &h[..h.rfind('<')?],
        None => head,
    };
    let name: String = head.chars().rev().take_while(|c| c.is_alphanumeric() || *c == '_').collect::<Vec<_>>().into_iter().rev().collect();
    let keywords = ["if", "for", "foreach", "while", "switch", "catch", "using", "return", "new", "nameof", "typeof", "lock"];
    (!name.is_empty() && !keywords.contains(&name.as_str()) && head.contains(' ')).then_some(name)
}

/// `--filter` for one test (every case of a theory), a class, or a whole project (`None`).
pub fn filter(full_name: &str, is_class: bool) -> String {
    let escaped: String = full_name.chars().flat_map(|c| if "()&|=!~\\".contains(c) { vec!['\\', c] } else { vec![c] }).collect();
    if is_class { format!("FullyQualifiedName~{escaped}.") } else { format!("FullyQualifiedName={escaped}") }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Passed,
    Failed,
    Skipped,
}

/// One result (a theory gives one per case).
#[derive(Clone, Debug, PartialEq)]
pub struct TestResult {
    /// `Namespace.Class.Method`, matching [`TestCase::full_name`].
    pub full_name: String,
    /// As the framework shows it (with theory arguments).
    pub display_name: String,
    pub outcome: Outcome,
    pub seconds: f64,
    pub message: Option<String>,
    pub stack_trace: Option<String>,
}

/// Results from a TRX file (`dotnet test --logger trx`).
pub fn parse_trx(xml: &str) -> anyhow::Result<Vec<TestResult>> {
    let doc = roxmltree::Document::parse(xml)?;
    let mut methods: HashMap<&str, String> = HashMap::new();
    for unit in doc.descendants().filter(|n| n.has_tag_name("UnitTest")) {
        let Some(id) = unit.attribute("id") else { continue };
        if let Some(method) = unit.children().find(|n| n.has_tag_name("TestMethod")) {
            let class = method.attribute("className").unwrap_or_default();
            // MSTest may add the assembly after a comma; NUnit puts the case arguments in the name.
            let class = class.split(',').next().unwrap_or(class).trim();
            let name = method.attribute("name").unwrap_or_default();
            let name = name.split('(').next().unwrap_or(name);
            methods.insert(id, format!("{class}.{name}"));
        }
    }
    let mut results = Vec::new();
    for r in doc.descendants().filter(|n| n.has_tag_name("UnitTestResult")) {
        let display = r.attribute("testName").unwrap_or_default().to_owned();
        let full_name = r.attribute("testId").and_then(|id| methods.get(id)).cloned().unwrap_or_else(|| display.split('(').next().unwrap_or(&display).to_owned());
        let outcome = match r.attribute("outcome").unwrap_or_default() {
            "Passed" => Outcome::Passed,
            "NotExecuted" | "Inconclusive" | "Pending" | "Disconnected" | "Warning" | "PassedButRunAborted" | "InProgress" => Outcome::Skipped,
            _ => Outcome::Failed,
        };
        let text_of = |tag: &str| r.descendants().find(|n| n.has_tag_name(tag)).and_then(|n| n.text()).map(|t| t.trim().to_owned()).filter(|t| !t.is_empty());
        results.push(TestResult {
            full_name,
            display_name: display,
            outcome,
            seconds: r.attribute("duration").map(parse_duration).unwrap_or(0.0),
            message: text_of("Message"),
            stack_trace: text_of("StackTrace"),
        });
    }
    Ok(results)
}

/// `00:00:01.2345678` → 1.2345678.
fn parse_duration(text: &str) -> f64 {
    text.split(':').fold(0.0, |acc, part| acc * 60.0 + part.parse::<f64>().unwrap_or(0.0))
}

/// The testhost's process id from `dotnet test` output with `VSTEST_HOST_DEBUG=1`
/// ("Process Id: 1234, Name: dotnet").
pub fn host_debug_pid(line: &str) -> Option<u32> {
    let rest = line.trim().strip_prefix("Process Id:")?;
    rest.split(',').next()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_xunit_file_scoped_namespace_and_nested_class() {
        let src = r#"
namespace XTests;

public class CalculatorTests
{
    [Fact]
    public void Adds() => Assert.Equal(4, 2 + 2);

    [Theory]
    [InlineData(1, 2)] // "{"
    public async Task Increments(int a, int b) { }

    public void Helper() { }

    public class Nested
    {
        [Fact(Skip = "later")]
        public void Skipped() { }
    }
}
"#;
        let classes = scan_source(src, Path::new("a.cs"));
        assert_eq!(classes.len(), 2);
        assert_eq!(classes[0].full_name, "XTests.CalculatorTests");
        let names: Vec<String> = classes[0].tests.iter().map(TestCase::full_name).collect();
        assert_eq!(names, ["XTests.CalculatorTests.Adds", "XTests.CalculatorTests.Increments"]);
        assert_eq!(classes[0].tests[0].line, 6);
        assert_eq!(classes[1].full_name, "XTests.CalculatorTests+Nested");
        assert_eq!(classes[1].short_name(), "CalculatorTests+Nested");
    }

    #[test]
    fn scans_nunit_and_mstest_block_namespaces() {
        let src = "namespace A.B\n{\n    [TestFixture]\n    public class T\n    {\n        [TestCase(1)]\n        [TestCase(2)]\n        public void Cases(int x)\n        {\n        }\n\n        [NUnit.Framework.Test]\n        public void Plain() {}\n    }\n}\nnamespace C {\n[TestClass] public sealed class M {\n [TestMethod]\n public void Go() {}\n}\n}\n";
        let classes = scan_source(src, Path::new("a.cs"));
        let names: Vec<String> = classes.iter().flat_map(|c| c.tests.iter().map(TestCase::full_name)).collect();
        assert_eq!(names, ["A.B.T.Cases", "A.B.T.Plain", "C.M.Go"]);
    }

    #[test]
    fn filters_escape_and_target_classes() {
        assert_eq!(filter("A.B.Test", false), "FullyQualifiedName=A.B.Test");
        assert_eq!(filter("A.Outer+Inner", true), "FullyQualifiedName~A.Outer+Inner.");
        assert_eq!(filter("A.B(x)", false), "FullyQualifiedName=A.B\\(x\\)");
    }

    #[test]
    fn parses_trx_results() {
        let trx = r#"<?xml version="1.0" encoding="utf-8"?>
<TestRun xmlns="http://microsoft.com/schemas/VisualStudio/TeamTest/2010">
  <Results>
    <UnitTestResult testId="1" testName="X.T.Inc(a: 1)" duration="00:00:01.5000000" outcome="Passed" />
    <UnitTestResult testId="2" testName="Cases(2)" duration="00:00:00.0100000" outcome="Failed">
      <Output><ErrorInfo><Message>Expected 1</Message><StackTrace>at X.T.Cases</StackTrace></ErrorInfo></Output>
    </UnitTestResult>
    <UnitTestResult testId="3" testName="X.T+N.S" duration="00:00:00" outcome="NotExecuted" />
  </Results>
  <TestDefinitions>
    <UnitTest id="1" name="X.T.Inc(a: 1)"><TestMethod className="X.T" name="Inc" /></UnitTest>
    <UnitTest id="2" name="Cases(2)"><TestMethod className="N.T, N, Version=1.0" name="Cases(2)" /></UnitTest>
    <UnitTest id="3" name="X.T+N.S"><TestMethod className="X.T+N" name="S" /></UnitTest>
  </TestDefinitions>
</TestRun>"#;
        let r = parse_trx(trx).unwrap();
        assert_eq!(r[0].full_name, "X.T.Inc");
        assert_eq!(r[0].outcome, Outcome::Passed);
        assert!((r[0].seconds - 1.5).abs() < 1e-9);
        assert_eq!(r[1].full_name, "N.T.Cases");
        assert_eq!(r[1].message.as_deref(), Some("Expected 1"));
        assert_eq!(r[2].outcome, Outcome::Skipped);
        assert_eq!(r[2].full_name, "X.T+N.S");
    }

    #[test]
    fn reads_host_debug_pid() {
        assert_eq!(host_debug_pid("Process Id: 66090, Name: dotnet"), Some(66090));
        assert_eq!(host_debug_pid("Starting test execution"), None);
    }
}
