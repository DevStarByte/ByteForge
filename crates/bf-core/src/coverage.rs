//! Code coverage from a test run: Cobertura XML, as written by coverlet (`--collect "XPlat Code Coverage"`)
//! and by Microsoft's collector (`--collect "Code Coverage;Format=cobertura"`), read into covered,
//! partly covered (some branches not taken) and uncovered lines per source file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// How a line was covered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineState {
    /// Ran (and every branch on it was taken).
    Covered,
    /// Ran, but not every branch on it was taken.
    Partial,
    /// Never ran.
    Uncovered,
}

/// One line's numbers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LineCoverage {
    pub hits: u64,
    /// Branches taken and branches there are, for a line with branches.
    pub branches: Option<(u32, u32)>,
}

impl LineCoverage {
    pub fn state(&self) -> LineState {
        match self.branches {
            _ if self.hits == 0 => LineState::Uncovered,
            Some((taken, total)) if taken < total => LineState::Partial,
            _ => LineState::Covered,
        }
    }

    /// The same line in another report (another test project's run): hits add up, the more branches taken wins.
    fn merge(&mut self, other: LineCoverage) {
        self.hits = self.hits.saturating_add(other.hits);
        self.branches = match (self.branches, other.branches) {
            (Some(a), Some(b)) => Some((a.0.max(b.0), a.1.max(b.1))),
            (a, b) => a.or(b),
        };
    }
}

/// One source file's lines (0-based line numbers).
#[derive(Clone, Debug, Default)]
pub struct FileCoverage {
    pub path: PathBuf,
    pub lines: BTreeMap<usize, LineCoverage>,
}

/// Lines and branches covered, of those there are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    pub lines_covered: usize,
    pub lines: usize,
    pub branches_covered: usize,
    pub branches: usize,
}

impl Totals {
    /// Percent of lines covered (100 when there are none).
    pub fn line_percent(&self) -> f64 {
        if self.lines == 0 { 100.0 } else { self.lines_covered as f64 * 100.0 / self.lines as f64 }
    }

    pub fn branch_percent(&self) -> f64 {
        if self.branches == 0 { 100.0 } else { self.branches_covered as f64 * 100.0 / self.branches as f64 }
    }

    fn add(&mut self, lines: &BTreeMap<usize, LineCoverage>) {
        for l in lines.values() {
            self.lines += 1;
            self.lines_covered += usize::from(l.hits > 0);
            if let Some((taken, total)) = l.branches {
                self.branches += total as usize;
                self.branches_covered += taken.min(total) as usize;
            }
        }
    }
}

/// The coverage of a run (one or more reports merged).
#[derive(Clone, Debug, Default)]
pub struct Coverage {
    /// By [`key`] of the path.
    files: BTreeMap<String, FileCoverage>,
}

impl Coverage {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn files(&self) -> impl Iterator<Item = &FileCoverage> {
        self.files.values()
    }

    /// A file's lines and how they were covered (0-based, in order); empty when the run didn't touch it.
    pub fn marks_for(&self, path: &Path) -> Vec<(usize, LineState)> {
        self.files.get(&key(path)).map(|f| f.lines.iter().map(|(&line, c)| (line, c.state())).collect()).unwrap_or_default()
    }

    pub fn file(&self, path: &Path) -> Option<&FileCoverage> {
        self.files.get(&key(path))
    }

    /// Adds another report (another test project's run) to this one.
    pub fn merge(&mut self, other: Coverage) {
        for (k, file) in other.files {
            let into = self.files.entry(k).or_insert_with(|| FileCoverage { path: file.path.clone(), lines: BTreeMap::new() });
            for (line, c) in file.lines {
                into.lines.entry(line).or_default().merge(c);
            }
        }
    }

    /// Lines and branches of the files `include` takes (all of them: `|_| true`).
    pub fn totals(&self, include: impl Fn(&Path) -> bool) -> Totals {
        let mut totals = Totals::default();
        for file in self.files.values().filter(|f| include(&f.path)) {
            totals.add(&file.lines);
        }
        totals
    }

    /// One file's lines and branches.
    pub fn file_totals(file: &FileCoverage) -> Totals {
        let mut totals = Totals::default();
        totals.add(&file.lines);
        totals
    }
}

/// A path as compared: case-insensitive on Windows and macOS, one kind of separator on Windows.
fn key(path: &Path) -> String {
    let s = path.to_string_lossy();
    if cfg!(windows) {
        s.replace('/', "\\").to_lowercase()
    } else if cfg!(target_os = "macos") {
        s.to_lowercase()
    } else {
        s.into_owned()
    }
}

/// Reads a Cobertura report. File names are absolute (Microsoft's collector) or relative to one of the
/// report's `<source>` folders (coverlet); a relative one is taken in the first source folder it exists in.
pub fn parse_cobertura(xml: &str) -> anyhow::Result<Coverage> {
    let doc = roxmltree::Document::parse(xml.trim_start_matches('\u{feff}'))?;
    if !doc.root_element().has_tag_name("coverage") {
        anyhow::bail!("not a Cobertura report");
    }
    let sources: Vec<PathBuf> = doc.descendants().filter(|n| n.has_tag_name("source")).filter_map(|n| n.text()).map(|t| PathBuf::from(t.trim())).collect();
    let resolve = |name: &str| -> PathBuf {
        let p = PathBuf::from(name);
        if p.is_absolute() || sources.is_empty() {
            return p;
        }
        sources.iter().map(|s| s.join(&p)).find(|full| full.exists()).unwrap_or_else(|| sources[0].join(&p))
    };
    let mut coverage = Coverage::default();
    for class in doc.descendants().filter(|n| n.has_tag_name("class")) {
        let Some(name) = class.attribute("filename").filter(|f| !f.is_empty()) else { continue };
        let path = resolve(name);
        let file = coverage.files.entry(key(&path)).or_insert_with(|| FileCoverage { path, lines: BTreeMap::new() });
        // The class's own <lines> (its methods repeat them).
        let Some(lines) = class.children().find(|n| n.has_tag_name("lines")) else { continue };
        for line in lines.children().filter(|n| n.has_tag_name("line")) {
            let Some(number) = line.attribute("number").and_then(|n| n.parse::<usize>().ok()).filter(|&n| n > 0) else { continue };
            let hits = line.attribute("hits").and_then(|h| h.parse::<u64>().ok()).unwrap_or(0);
            let branches = (line.attribute("branch") == Some("True")).then(|| line.attribute("condition-coverage").and_then(parse_condition)).flatten();
            file.lines.entry(number - 1).or_default().merge(LineCoverage { hits, branches });
        }
    }
    Ok(coverage)
}

/// `"50% (1/2)"` → (1, 2).
fn parse_condition(text: &str) -> Option<(u32, u32)> {
    let inner = text.split('(').nth(1)?.trim_end_matches(')');
    let (taken, total) = inner.split_once('/')?;
    Some((taken.trim().parse().ok()?, total.trim().parse().ok()?))
}

/// Every Cobertura report under `dir` (the test run's results folder; collectors write each into a folder of
/// its own), merged.
pub fn read_reports(dir: &Path) -> Coverage {
    let mut coverage = Coverage::default();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for path in entries.flatten().map(|e| e.path()) {
            if path.is_dir() {
                stack.push(path);
            } else if path.to_string_lossy().ends_with(".cobertura.xml") {
                match std::fs::read_to_string(&path).map_err(anyhow::Error::from).and_then(|xml| parse_cobertura(&xml)) {
                    Ok(c) => coverage.merge(c),
                    Err(e) => log::warn!("could not read coverage {}: {e:#}", path.display()),
                }
            }
        }
    }
    coverage
}

/// The `--collect` argument for a test project: coverlet when the project references `coverlet.collector`
/// (the xunit, NUnit and MSTest templates do; it leaves the test project out), else Microsoft's collector,
/// which comes with Microsoft.NET.Test.Sdk.
pub fn collector_for(project: &Path) -> &'static str {
    let text = std::fs::read_to_string(project).unwrap_or_default().to_lowercase();
    if text.contains("coverlet.collector") { "XPlat Code Coverage" } else { "Code Coverage;Format=cobertura" }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MICROSOFT: &str = "\u{feff}<?xml version=\"1.0\" encoding=\"utf-8\"?>
<coverage line-rate=\"0.45\" branch-rate=\"0.5\" version=\"1.9\">
  <packages>
    <package name=\"Lib\">
      <classes>
        <class name=\"Lib.Calc\" filename=\"ABS/Lib/Calc.cs\">
          <methods>
            <method name=\"Abs\"><lines><line number=\"6\" hits=\"1\" branch=\"False\" /></lines></method>
          </methods>
          <lines>
            <line number=\"6\" hits=\"1\" branch=\"False\" />
            <line number=\"7\" hits=\"1\" branch=\"True\" condition-coverage=\"50% (1/2)\"><conditions><condition number=\"0\" type=\"jump\" coverage=\"50%\" /></conditions></line>
            <line number=\"8\" hits=\"0\" branch=\"False\" />
            <line number=\"11\" hits=\"3\" branch=\"True\" condition-coverage=\"100% (2/2)\" />
          </lines>
        </class>
      </classes>
    </package>
  </packages>
</coverage>";

    fn abs() -> PathBuf {
        std::env::temp_dir().join("bf-cov")
    }

    #[test]
    fn reads_lines_branches_and_states() {
        let xml = MICROSOFT.replace("ABS", &abs().to_string_lossy());
        let c = parse_cobertura(&xml).unwrap();
        let file = abs().join("Lib").join("Calc.cs");
        // 0-based lines; the method's repeated lines aren't counted twice.
        assert_eq!(c.marks_for(&file), vec![(5, LineState::Covered), (6, LineState::Partial), (7, LineState::Uncovered), (10, LineState::Covered)]);
        let t = c.totals(|_| true);
        assert_eq!(t, Totals { lines_covered: 3, lines: 4, branches_covered: 3, branches: 4 });
        assert_eq!(t.line_percent(), 75.0);
        // Another run of the same file: hits add up, a branch taken there counts.
        let other = parse_cobertura(&xml.replace("hits=\"0\"", "hits=\"2\"").replace("50% (1/2)", "100% (2/2)")).unwrap();
        let mut merged = c.clone();
        merged.merge(other);
        assert!(merged.marks_for(&file).iter().all(|(_, s)| *s == LineState::Covered));
        assert_eq!(merged.file(&file).unwrap().lines[&5].hits, 2);
    }

    #[test]
    fn relative_file_names_are_taken_in_their_source_folder() {
        let root = abs().join("rel");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src").join("A.cs"), "").unwrap();
        let xml = format!(
            "<coverage><sources><source>{}</source><source>{}</source></sources><packages><package><classes><class filename=\"A.cs\"><lines><line number=\"1\" hits=\"0\"/></lines></class></classes></package></packages></coverage>",
            root.join("nowhere").display(),
            root.join("src").display()
        );
        let c = parse_cobertura(&xml).unwrap();
        assert_eq!(c.marks_for(&root.join("src").join("A.cs")), vec![(0, LineState::Uncovered)]);
        assert!(parse_cobertura("<TestRun/>").is_err());
    }

    #[test]
    fn paths_are_compared_like_the_file_system() {
        let xml = MICROSOFT.replace("ABS", &abs().to_string_lossy());
        let c = parse_cobertura(&xml).unwrap();
        let upper = PathBuf::from(abs().join("Lib").join("Calc.cs").to_string_lossy().to_uppercase());
        assert_eq!(!c.marks_for(&upper).is_empty(), cfg!(any(windows, target_os = "macos")));
    }

    #[test]
    fn condition_coverage() {
        assert_eq!(parse_condition("50% (1/2)"), Some((1, 2)));
        assert_eq!(parse_condition("100% (4/4)"), Some((4, 4)));
        assert_eq!(parse_condition("50%"), None);
    }
}
