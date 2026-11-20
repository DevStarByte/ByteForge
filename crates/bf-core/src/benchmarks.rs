//! BenchmarkDotNet: the `[Benchmark]` methods of a file (found in the source, like tests), the command
//! that runs one in Release, and the results its runs leave in
//! `BenchmarkDotNet.Artifacts/results/*-report-full.json`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// A class with `[Benchmark]` methods.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BenchmarkClass {
    pub namespace: Option<String>,
    pub name: String,
    /// The line of the class declaration (0-based).
    pub line: usize,
    /// (method name, line of its declaration).
    pub methods: Vec<(String, usize)>,
}

impl BenchmarkClass {
    pub fn full_name(&self) -> String {
        match &self.namespace {
            Some(ns) => format!("{ns}.{}", self.name),
            None => self.name.clone(),
        }
    }
}

/// The benchmark classes of a C# file: methods with a `[Benchmark]` or `[Benchmark(…)]` attribute, under
/// the class declared above them.
pub fn scan_source(text: &str) -> Vec<BenchmarkClass> {
    let mut out: Vec<BenchmarkClass> = Vec::new();
    let mut namespace: Option<String> = None;
    let mut class: Option<(String, usize)> = None;
    let mut pending = false;
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.starts_with("//") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("namespace ") {
            namespace = Some(rest.trim_end_matches([';', '{', ' ']).trim().to_owned());
            continue;
        }
        if let Some(name) = declared_class(line) {
            class = Some((name, i));
            pending = false;
            continue;
        }
        if is_benchmark_attribute(line) {
            pending = true;
            // `[Benchmark] public void Run() { }` on one line.
            let after = &line[line.find(']').map_or(line.len(), |j| j + 1)..];
            if let Some(name) = method_name(after) {
                add(&mut out, &namespace, &class, name, i);
                pending = false;
            }
            continue;
        }
        if pending && !line.is_empty() && !line.starts_with('[') {
            if let Some(name) = method_name(line) {
                add(&mut out, &namespace, &class, name, i);
            }
            pending = false;
        }
    }
    out
}

fn add(out: &mut Vec<BenchmarkClass>, namespace: &Option<String>, class: &Option<(String, usize)>, method: String, line: usize) {
    let Some((name, class_line)) = class else { return };
    match out.iter_mut().find(|c| &c.name == name && c.line == *class_line) {
        Some(c) => c.methods.push((method, line)),
        None => out.push(BenchmarkClass { namespace: namespace.clone(), name: name.clone(), line: *class_line, methods: vec![(method, line)] }),
    }
}

fn is_benchmark_attribute(line: &str) -> bool {
    let Some(inner) = line.strip_prefix('[') else { return false };
    inner.split([',', ']']).any(|a| {
        let a = a.trim();
        let name = a.split('(').next().unwrap_or_default().trim();
        matches!(name, "Benchmark" | "BenchmarkAttribute" | "BenchmarkDotNet.Attributes.Benchmark")
    })
}

fn declared_class(line: &str) -> Option<String> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let at = words.iter().position(|w| *w == "class")?;
    let name = words.get(at + 1)?;
    let name: String = name.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
    (!name.is_empty() && words[..at].iter().all(|w| matches!(*w, "public" | "internal" | "sealed" | "partial" | "static" | "abstract" | "file" | "private" | "protected")))
        .then_some(name)
}

/// The name of the method declared on `line` (`public void Sort()`, `public async Task<int> Read(…)`).
fn method_name(line: &str) -> Option<String> {
    let open = line.find('(')?;
    let head = line[..open].trim_end();
    let name: String = head.chars().rev().take_while(|c| c.is_alphanumeric() || *c == '_').collect::<Vec<_>>().into_iter().rev().collect();
    let generic = head.ends_with('>');
    (!name.is_empty() && !generic && head.split_whitespace().count() >= 2).then_some(name)
}

/// What a click on "Run Benchmark" runs: a method, or every benchmark of a class.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BenchmarkRun {
    /// `Namespace.Class.Method` or `Namespace.Class`.
    pub full_name: String,
    pub method: Option<String>,
}

impl BenchmarkRun {
    /// BenchmarkDotNet's `--filter` glob for it.
    pub fn filter(&self) -> String {
        match &self.method {
            Some(_) => format!("*{}*", self.full_name),
            None => format!("*{}.*", self.full_name),
        }
    }
}

/// What "Run Benchmark at Cursor" runs for a caret on `line`: a method declared just below (the caret is on
/// its attributes), else the nearest one declared above (the caret is in its body), else — above the first
/// method — every benchmark of the class.
pub fn run_at(classes: &[BenchmarkClass], line: usize) -> Option<BenchmarkRun> {
    let class = classes.iter().filter(|c| c.line <= line).max_by_key(|c| c.line)?;
    let below = class.methods.iter().filter(|(_, l)| *l > line && *l - line <= 3).min_by_key(|(_, l)| *l);
    let method = below.or_else(|| class.methods.iter().filter(|(_, l)| *l <= line).max_by_key(|(_, l)| *l));
    Some(match method {
        Some((name, _)) => BenchmarkRun { full_name: format!("{}.{name}", class.full_name()), method: Some(name.clone()) },
        None => BenchmarkRun { full_name: class.full_name(), method: None },
    })
}

/// `dotnet run -c Release --project <project> -- --filter <glob>`. BenchmarkDotNet only reads the filter
/// when `Main` passes its arguments on (`BenchmarkSwitcher.FromAssembly(…).Run(args)`).
pub fn command(project: &Path, run: &BenchmarkRun) -> String {
    format!("dotnet run -c Release --project {} -- --filter {}", quote(&project.to_string_lossy()), quote(&run.filter()))
}

fn quote(s: &str) -> String {
    if s.chars().all(|c| c.is_alphanumeric() || matches!(c, '/' | '.' | '-' | '_' | ':' | '\\')) {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// One benchmark's result, from a full JSON report.
#[derive(Clone, Debug, PartialEq)]
pub struct BenchmarkResult {
    /// `Namespace.Class.Method(N: 10)`: identifies it across runs.
    pub full_name: String,
    /// `Class.Method` plus its parameters (`N=10`) when it has any.
    pub display: String,
    /// Nanoseconds; `None` when the benchmark failed.
    pub mean: Option<f64>,
    /// Half the 99.9% confidence interval, in nanoseconds (BenchmarkDotNet's "Error").
    pub error: Option<f64>,
    /// Bytes per operation (with `[MemoryDiagnoser]`).
    pub allocated: Option<f64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Report {
    #[serde(default)]
    benchmarks: Vec<ReportBenchmark>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ReportBenchmark {
    #[serde(default)]
    full_name: String,
    #[serde(default, rename = "Type")]
    type_name: String,
    #[serde(default)]
    method: String,
    #[serde(default)]
    parameters: String,
    statistics: Option<Statistics>,
    memory: Option<Memory>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Statistics {
    mean: Option<f64>,
    confidence_interval: Option<Interval>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Interval {
    margin: Option<f64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Memory {
    bytes_allocated_per_operation: Option<f64>,
}

/// The results of a `*-report-full.json`.
pub fn parse_report(text: &str) -> Vec<BenchmarkResult> {
    let Ok(report) = serde_json::from_str::<Report>(text.trim_start_matches('\u{feff}')) else { return Vec::new() };
    report
        .benchmarks
        .into_iter()
        .map(|b| {
            let mut display = format!("{}.{}", b.type_name, b.method);
            if !b.parameters.is_empty() {
                display.push_str(&format!(" ({})", b.parameters.replace('&', ", ")));
            }
            let full_name = if b.full_name.is_empty() { display.clone() } else { b.full_name };
            BenchmarkResult {
                full_name,
                display,
                mean: b.statistics.as_ref().and_then(|s| s.mean),
                error: b.statistics.as_ref().and_then(|s| s.confidence_interval.as_ref()?.margin),
                allocated: b.memory.and_then(|m| m.bytes_allocated_per_operation),
            }
        })
        .collect()
}

/// The full JSON reports below `root` (in `BenchmarkDotNet.Artifacts/results` folders, up to four folders
/// down), with when each was written.
pub fn find_reports(root: &Path) -> Vec<(PathBuf, SystemTime)> {
    let mut out = Vec::new();
    let mut dirs = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = dirs.pop() {
        let results = dir.join("BenchmarkDotNet.Artifacts").join("results");
        if let Ok(entries) = std::fs::read_dir(&results) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.ends_with("-report-full.json")
                    && let Ok(modified) = e.metadata().and_then(|m| m.modified())
                {
                    out.push((e.path(), modified));
                }
            }
        }
        if depth >= 4 {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.file_type().is_ok_and(|t| t.is_dir()) && !name.starts_with('.') && !matches!(name.as_str(), "bin" | "obj" | "node_modules" | "BenchmarkDotNet.Artifacts" | "target") {
                dirs.push((e.path(), depth + 1));
            }
        }
    }
    out
}

/// What the last runs measured, per benchmark: kept between sessions so a new run can show the change.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct History(HashMap<String, Measured>);

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Measured {
    pub mean: f64,
    /// The run before (what the change compares to).
    pub previous: Option<f64>,
    /// When the report with `mean` was written (seconds since the epoch): a newer report is a new run.
    pub stamp: u64,
}

impl History {
    /// Takes in a report written at `written`: a benchmark whose last report is older moves its mean to
    /// `previous`. Returns whether anything changed.
    pub fn record(&mut self, results: &[BenchmarkResult], written: SystemTime) -> bool {
        let stamp = written.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let mut changed = false;
        for r in results {
            let Some(mean) = r.mean else { continue };
            match self.0.get_mut(&r.full_name) {
                Some(m) if m.stamp >= stamp => {}
                Some(m) => {
                    *m = Measured { mean, previous: Some(m.mean), stamp };
                    changed = true;
                }
                None => {
                    self.0.insert(r.full_name.clone(), Measured { mean, previous: None, stamp });
                    changed = true;
                }
            }
        }
        changed
    }

    /// The change of `result` since the run before, as a fraction (−0.12: 12% faster).
    pub fn change(&self, result: &BenchmarkResult) -> Option<f64> {
        let m = self.0.get(&result.full_name)?;
        let previous = m.previous.filter(|p| *p > 0.0)?;
        Some((result.mean? - previous) / previous)
    }
}

/// `1.234 ns`, `12.35 μs`, `1.500 ms`, `2.000 s`.
pub fn format_time(ns: f64) -> String {
    let (value, unit) = if ns < 1e3 {
        (ns, "ns")
    } else if ns < 1e6 {
        (ns / 1e3, "μs")
    } else if ns < 1e9 {
        (ns / 1e6, "ms")
    } else {
        (ns / 1e9, "s")
    };
    let digits = if value < 10.0 { 3 } else if value < 100.0 { 2 } else { 1 };
    format!("{value:.digits$} {unit}")
}

/// `0 B`, `120 B`, `1.46 KB`, `3.2 MB`.
pub fn format_bytes(bytes: f64) -> String {
    if bytes < 1024.0 {
        format!("{bytes:.0} B")
    } else if bytes < 1024.0 * 1024.0 {
        format!("{:.2} KB", bytes / 1024.0)
    } else {
        format!("{:.2} MB", bytes / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = r#"using BenchmarkDotNet.Attributes;

namespace Shop.Benchmarks;

[MemoryDiagnoser]
public class SortBenchmarks
{
    [Params(10, 100)]
    public int N;

    [Benchmark(Baseline = true)]
    public void ArraySort() => Array.Sort(data);

    [Benchmark]
    [Arguments(3)]
    public async Task<int> LinqOrder(int x) { return 0; }

    public void NotABenchmark() { }

    [Benchmark] public int Inline() => 1;
}
"#;

    #[test]
    fn finds_benchmark_methods_under_their_class() {
        let classes = scan_source(SOURCE);
        assert_eq!(classes.len(), 1);
        let c = &classes[0];
        assert_eq!(c.full_name(), "Shop.Benchmarks.SortBenchmarks");
        assert_eq!(c.line, 5);
        assert_eq!(c.methods, [("ArraySort".to_owned(), 11), ("LinqOrder".to_owned(), 15), ("Inline".to_owned(), 19)]);
        assert!(scan_source("public class Plain { public void Run() {} }").is_empty());
    }

    #[test]
    fn runs_at_a_method_or_its_class() {
        let classes = scan_source(SOURCE);
        let at = |line| run_at(&classes, line).map(|r| (r.full_name.clone(), r.filter()));
        assert_eq!(at(11), Some(("Shop.Benchmarks.SortBenchmarks.ArraySort".into(), "*Shop.Benchmarks.SortBenchmarks.ArraySort*".into())));
        // The attribute line above the method counts too.
        assert_eq!(at(10).unwrap().0, "Shop.Benchmarks.SortBenchmarks.ArraySort");
        assert_eq!(at(5), Some(("Shop.Benchmarks.SortBenchmarks".into(), "*Shop.Benchmarks.SortBenchmarks.*".into())));
        assert_eq!(at(0), None);
        let run = run_at(&classes, 11).unwrap();
        assert_eq!(command(Path::new("/w/Bench/Bench.csproj"), &run), "dotnet run -c Release --project /w/Bench/Bench.csproj -- --filter '*Shop.Benchmarks.SortBenchmarks.ArraySort*'");
    }

    #[test]
    fn reads_a_full_report() {
        let json = r#"{"Title":"x","Benchmarks":[
            {"FullName":"Shop.SortBenchmarks.ArraySort(N: 10)","Type":"SortBenchmarks","Method":"ArraySort","Parameters":"N=10",
             "Statistics":{"Mean":1234.5,"ConfidenceInterval":{"Margin":12.5}},"Memory":{"BytesAllocatedPerOperation":0}},
            {"FullName":"Shop.SortBenchmarks.Broken","Type":"SortBenchmarks","Method":"Broken","Parameters":"","Statistics":null,"Memory":null}]}"#;
        let results = parse_report(json);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].display, "SortBenchmarks.ArraySort (N=10)");
        assert_eq!(results[0].mean, Some(1234.5));
        assert_eq!(results[0].error, Some(12.5));
        assert_eq!(results[0].allocated, Some(0.0));
        assert_eq!(results[1].mean, None);
        assert!(parse_report("nope").is_empty());
    }

    #[test]
    fn history_keeps_the_run_before() {
        let r = |mean| BenchmarkResult { full_name: "A.B".into(), display: "A.B".into(), mean: Some(mean), error: None, allocated: None };
        let t = |s| SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(s);
        let mut h = History::default();
        assert!(h.record(&[r(100.0)], t(10)));
        assert_eq!(h.change(&r(100.0)), None, "no run before the first");
        assert!(!h.record(&[r(100.0)], t(10)), "the same report again changes nothing");
        assert!(h.record(&[r(80.0)], t(20)));
        let change = h.change(&r(80.0)).unwrap();
        assert!((change + 0.2).abs() < 1e-9, "20% faster: {change}");
        assert!(!h.record(&[r(999.0)], t(15)), "an older report doesn't count");
    }

    #[test]
    fn formats_times_and_sizes() {
        assert_eq!(format_time(1.2345), "1.234 ns");
        assert_eq!(format_time(12_345.0), "12.35 μs");
        assert_eq!(format_time(1_500_000.0), "1.500 ms");
        assert_eq!(format_bytes(120.0), "120 B");
        assert_eq!(format_bytes(1500.0), "1.46 KB");
    }

    #[test]
    fn finds_reports_in_artifact_folders() {
        let dir = std::env::temp_dir().join(format!("bf-bench-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let results = dir.join("Bench/BenchmarkDotNet.Artifacts/results");
        std::fs::create_dir_all(&results).unwrap();
        std::fs::write(results.join("Shop.SortBenchmarks-report-full.json"), "{}").unwrap();
        std::fs::write(results.join("Shop.SortBenchmarks-report.csv"), "").unwrap();
        let found = find_reports(&dir);
        assert_eq!(found.len(), 1);
        assert!(found[0].0.ends_with("Shop.SortBenchmarks-report-full.json"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
