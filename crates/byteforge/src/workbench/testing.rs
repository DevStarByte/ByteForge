//! Tests in the workbench: running and debugging them with `dotnet test`, reading the TRX results back
//! into the Test Explorer, and the pass/fail markers in the editors' gutters. Run with coverage, the
//! collector's Cobertura reports are read back into coverage bars in the gutters.

use bf_core::testing::{self as tst, Outcome, TestResult};

use super::*;
use crate::testing::{TestEvent, TestRun, TestView};

impl Workbench {
    pub(super) fn on_test_event(&mut self, _: &Entity<TestView>, event: &TestEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            TestEvent::Run(runs) => self.run_tests(runs.clone(), false, false, window, cx),
            TestEvent::RunWithCoverage(runs) => self.run_tests(runs.clone(), false, true, window, cx),
            TestEvent::Debug(run) => self.run_tests(vec![run.clone()], true, false, window, cx),
            TestEvent::OpenFile(path, line) => self.open_file(path.clone(), Some(Point::new(*line, 0)), window, cx),
            TestEvent::Changed => self.refresh_test_marks(cx),
        }
    }

    /// Runs the queue one `dotnet test` after another. With `debug`, the test host waits for the debugger
    /// (`VSTEST_HOST_DEBUG=1`) and is attached to as soon as it prints its process id. With `coverage`,
    /// each run collects code coverage and, once the queue is done, the gutters show it.
    pub(super) fn run_tests(&mut self, mut queue: Vec<TestRun>, debug: bool, coverage: bool, window: &mut Window, cx: &mut Context<Self>) {
        if queue.is_empty() {
            return;
        }
        let Some(root) = self.workspace.root() else { return };
        if self.dotnet_busy(cx) {
            return;
        }
        self.save_all(window, cx);
        let run = queue.remove(0);
        self.test_runs += 1;
        let results_dir = std::env::temp_dir().join(format!("byteforge-tests-{}-{}", std::process::id(), self.test_runs));
        let _ = std::fs::remove_dir_all(&results_dir);
        let mut args: Vec<String> = vec![
            "test".into(),
            run.project.to_string_lossy().into_owned(),
            "--logger".into(),
            "trx".into(),
            // Each test as it finishes, for live progress in the Test Explorer.
            "--logger".into(),
            "console;verbosity=normal".into(),
            "--results-directory".into(),
            results_dir.to_string_lossy().into_owned(),
        ];
        if let Some(filter) = &run.filter {
            args.extend(["--filter".into(), filter.clone()]);
        }
        if coverage {
            args.extend(["--collect".into(), bf_core::coverage::collector_for(&run.project).into()]);
        }
        self.tests.update(cx, |v, cx| v.start_run(&run, cx));
        self.show_panel(PanelView::Output, cx);
        self.log(format!("{} tests{}: {}", if debug { "Debugging" } else { "Running" }, if coverage { " with coverage" } else { "" }, run.label));

        let env: Vec<(String, String)> = if debug { vec![("VSTEST_HOST_DEBUG".into(), "1".into())] } else { Vec::new() };
        let mut attached = false;
        let on_line: Option<LineHook> = Some(Box::new(move |wb: &mut Workbench, line: &str, window: &mut Window, cx: &mut Context<Workbench>| {
            if let Some((outcome, name)) = tst::parse_progress_line(line) {
                wb.tests.update(cx, |v, cx| v.live_result(outcome, &name, cx));
            }
            if debug && !attached && let Some(pid) = tst::host_debug_pid(line) {
                attached = true;
                wb.debug_attach(pid, window, cx);
            }
        }));
        let label = run.label.clone();
        let then: DotnetDone = Box::new(move |wb, _, window, cx| {
            let results = read_results(&results_dir);
            if coverage {
                let report = bf_core::coverage::read_reports(&results_dir);
                wb.coverage_pending.get_or_insert_default().merge(report);
            }
            let _ = std::fs::remove_dir_all(&results_dir);
            if wb.dotnet_stopped {
                wb.tests.update(cx, |v, cx| v.cancel_run(cx));
                if !queue.is_empty() {
                    wb.log(format!("Tests: stopped; {} more test project(s) not run.", queue.len()));
                }
                if coverage {
                    wb.finish_coverage(cx);
                }
                return;
            }
            if results.is_empty() {
                // Most likely this project didn't build; the other projects can still run.
                wb.log(format!("Tests: no results for {label} (did the build fail? see above)."));
                wb.tests.update(cx, |v, cx| v.cancel_run(cx));
            } else {
                wb.log(summary_line(&label, &results));
                wb.tests.update(cx, |v, cx| v.finish_run(results, cx));
            }
            if !queue.is_empty() {
                wb.run_tests(queue, false, coverage, window, cx);
            } else if coverage {
                wb.finish_coverage(cx);
            }
        });
        if !self.start_dotnet_with(root, args, env, on_line, Some(then), window, cx) {
            self.tests.update(cx, |v, cx| v.cancel_run(cx));
            self.coverage_pending = None;
        }
    }

    pub(super) fn run_all_tests_with_coverage(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_tests_discovered(cx);
        let runs = self.tests.read(cx).run_all();
        if runs.is_empty() {
            self.log("Coverage: no test projects found (a test project references Microsoft.NET.Test.Sdk).");
            self.show_panel(PanelView::Output, cx);
            return;
        }
        self.run_tests(runs, false, true, window, cx);
    }

    pub(super) fn run_test_at_cursor_with_coverage(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.active else { return };
        let Some(path) = self.tabs[ix].path.clone() else { return };
        let line = self.tabs[ix].editor.read(cx).state.caret().line;
        self.ensure_tests_discovered(cx);
        match self.tests.read(cx).run_at(&path, line) {
            Some(run) => self.run_tests(vec![run], false, true, window, cx),
            None => {
                self.log("No test at the cursor (tests are methods with [Fact], [Theory], [Test], [TestCase] or [TestMethod]).");
                self.show_panel(PanelView::Output, cx);
            }
        }
    }

    /// A coverage run is done: its coverage replaces the last one, the gutters show it and the Output sums it up.
    fn finish_coverage(&mut self, cx: &mut Context<Self>) {
        let coverage = self.coverage_pending.take().unwrap_or_default();
        if coverage.is_empty() {
            self.log(
                "Coverage: no coverage report was written. The test project needs Microsoft.NET.Test.Sdk (Microsoft's \
                 collector comes with it) or the coverlet.collector package.",
            );
            return;
        }
        let test_dirs = self.tests.read(cx).project_dirs();
        let in_tests = |p: &Path| test_dirs.iter().any(|d| p.ancestors().any(|a| bf_core::paths::same_path(a, d)));
        self.log(coverage_summary(&coverage, &in_tests, self.workspace.root().as_deref()));
        self.coverage = Some(coverage);
        self.coverage_visible = true;
        self.refresh_coverage_marks(cx);
    }

    /// The coverage bars of a file (none while hidden, or when the run didn't touch it).
    pub(super) fn coverage_marks_for(&self, path: &Path) -> Vec<(usize, bf_core::coverage::LineState)> {
        match &self.coverage {
            Some(c) if self.coverage_visible => c.marks_for(path),
            _ => Vec::new(),
        }
    }

    pub(super) fn refresh_coverage_marks(&mut self, cx: &mut Context<Self>) {
        let marks: Vec<_> = self.tabs.iter().map(|t| t.path.as_deref().map(|p| self.coverage_marks_for(p)).unwrap_or_default()).collect();
        for (tab, marks) in self.tabs.iter().zip(marks) {
            tab.editor.update(cx, |e, cx| e.set_coverage_marks(marks, cx));
        }
    }

    pub(super) fn toggle_coverage(&mut self, cx: &mut Context<Self>) {
        if self.coverage.is_none() {
            self.log("No coverage yet: run the tests with coverage first (Test → Run All Tests with Coverage).");
            self.show_panel(PanelView::Output, cx);
            return;
        }
        self.coverage_visible = !self.coverage_visible;
        self.refresh_coverage_marks(cx);
    }

    pub(super) fn clear_coverage(&mut self, cx: &mut Context<Self>) {
        self.coverage = None;
        self.refresh_coverage_marks(cx);
    }

    pub(super) fn run_all_tests(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_tests_discovered(cx);
        let runs = self.tests.read(cx).run_all();
        if runs.is_empty() {
            // Not discovered yet, or no test project: fall back to plain `dotnet test`.
            self.run_dotnet("test", window, cx);
            return;
        }
        self.run_tests(runs, false, false, window, cx);
    }

    pub(super) fn rerun_failed_tests(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let runs = self.tests.read(cx).run_failed();
        if runs.is_empty() {
            self.log("No failed tests to rerun.");
            self.show_panel(PanelView::Output, cx);
            return;
        }
        self.run_tests(runs, false, false, window, cx);
    }

    /// Runs (or debugs) the test or test class at the caret (the nearest declaration above it).
    pub(super) fn run_test_at_cursor(&mut self, debug: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.active else { return };
        let Some(path) = self.tabs[ix].path.clone() else { return };
        let line = self.tabs[ix].editor.read(cx).state.caret().line;
        self.run_test_at(&path, line, debug, window, cx);
    }

    pub(super) fn run_test_at(&mut self, path: &Path, line: usize, debug: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_tests_discovered(cx);
        match self.tests.read(cx).run_at(path, line) {
            Some(run) => self.run_tests(vec![run], debug, false, window, cx),
            None => {
                self.log("No test at the cursor (tests are methods with [Fact], [Theory], [Test], [TestCase] or [TestMethod]).");
                self.show_panel(PanelView::Output, cx);
            }
        }
    }

    /// Discovery normally runs when the Testing view is first shown; commands can come earlier.
    fn ensure_tests_discovered(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.root() else { return };
        if !self.tests.read(cx).is_discovered() {
            let projects = tst::discover(&root);
            self.tests.update(cx, |v, cx| v.set_projects(projects, cx));
        }
    }

    /// Pushes the Test Explorer's states to the gutters of the open editors.
    pub(super) fn refresh_test_marks(&mut self, cx: &mut Context<Self>) {
        let tests = self.tests.read(cx);
        let marks: Vec<_> = self.tabs.iter().map(|t| t.path.as_deref().map(|p| tests.marks_for(p)).unwrap_or_default()).collect();
        for (tab, marks) in self.tabs.iter().zip(marks) {
            tab.editor.update(cx, |e, cx| e.set_test_marks(marks, cx));
        }
    }
}

/// Every result in the TRX files of a results directory.
fn read_results(dir: &Path) -> Vec<TestResult> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut results = Vec::new();
    for path in entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "trx")) {
        match std::fs::read_to_string(&path).map_err(anyhow::Error::from).and_then(|xml| tst::parse_trx(&xml)) {
            Ok(r) => results.extend(r),
            Err(e) => log::warn!("could not read {}: {e:#}", path.display()),
        }
    }
    results
}

/// "Coverage: 78.4% of lines (…), 50.0% of branches (…)" for the code under test (not the test projects), and
/// the files with the least coverage.
fn coverage_summary(coverage: &bf_core::coverage::Coverage, in_tests: &dyn Fn(&Path) -> bool, root: Option<&Path>) -> String {
    use bf_core::coverage::Coverage;
    let totals = coverage.totals(|p| !in_tests(p));
    let mut line = format!("Coverage: {:.1}% of lines ({} of {})", totals.line_percent(), totals.lines_covered, totals.lines);
    if totals.branches > 0 {
        line.push_str(&format!(", {:.1}% of branches ({} of {})", totals.branch_percent(), totals.branches_covered, totals.branches));
    }
    let mut files: Vec<_> = coverage.files().filter(|f| !in_tests(&f.path) && !f.lines.is_empty()).map(|f| (Coverage::file_totals(f), &f.path)).collect();
    files.sort_by(|a, b| a.0.line_percent().total_cmp(&b.0.line_percent()).then_with(|| a.1.cmp(b.1)));
    let below: Vec<_> = files.iter().filter(|(t, _)| t.lines_covered < t.lines).take(10).collect();
    if !below.is_empty() {
        line.push_str("\n  Least covered:");
        for (t, path) in below {
            let shown = root.and_then(|r| path.strip_prefix(r).ok()).unwrap_or(path);
            line.push_str(&format!("\n  {:>5.1}%  {} ({} of {} lines)", t.line_percent(), shown.display(), t.lines_covered, t.lines));
        }
    }
    line
}

fn summary_line(label: &str, results: &[TestResult]) -> String {
    let count = |o: Outcome| results.iter().filter(|r| r.outcome == o).count();
    let (passed, failed, skipped) = (count(Outcome::Passed), count(Outcome::Failed), count(Outcome::Skipped));
    let mut line = format!("Tests ({label}): {passed} passed, {failed} failed");
    if skipped > 0 {
        line.push_str(&format!(", {skipped} skipped"));
    }
    for r in results.iter().filter(|r| r.outcome == Outcome::Failed).take(10) {
        line.push_str(&format!("\n  ✗ {}: {}", r.display_name, r.message.as_deref().unwrap_or("failed").lines().next().unwrap_or_default()));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestState;
    use crate::workbench::settings_page::view_tests::workbench;

    const TESTS: &str = "namespace XT;

public class CalcTests
{
    [Fact]
    public void Adds() => Assert.Equal(4, 2 + 2);

    [Fact]
    public void Fails() => Assert.Equal(5, 2 + 2);

    [Theory]
    [InlineData(1)]
    [InlineData(2)]
    public void Positive(int x) => Assert.True(x > 0);
}

// Another class runs in parallel: the CalcTests finish while this one still runs.
public class SlowTests
{
    [Fact]
    public void Slow() => Thread.Sleep(4000);
}
";

    fn dotnet(dir: &Path, args: &[&str]) -> bool {
        std::process::Command::new("dotnet").args(args).current_dir(dir).env("DOTNET_NOLOGO", "1").output().is_ok_and(|o| o.status.success())
    }

    fn click(cx: &mut gpui::VisualTestContext, name: &'static str) {
        cx.run_until_parked();
        let bounds = cx.debug_bounds(name).unwrap_or_else(|| panic!("{name} is drawn"));
        cx.simulate_click(bounds.center(), gpui::Modifiers::none());
        cx.run_until_parked();
    }

    fn output(wb: &Workbench, cx: &App) -> String {
        wb.output.read(cx).state.buffer.text()
    }

    /// Polls until `done` (the run's `dotnet test` is a real process), at most two minutes.
    fn wait(cx: &mut gpui::VisualTestContext, wb: &Entity<Workbench>, mut done: impl FnMut(&Workbench, &App) -> bool) {
        for _ in 0..1200 {
            cx.run_until_parked();
            if wb.read_with(cx, |wb, cx| done(wb, cx)) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("timed out; output:\n{}", wb.read_with(cx, output));
    }

    fn state(wb: &Workbench, cx: &App, test: &str) -> TestState {
        wb.tests.read(cx).test_state(&format!("XT.{test}"))
    }

    fn summary(cx: &mut gpui::VisualTestContext, wb: &Entity<Workbench>) -> String {
        wb.read_with(cx, |wb, cx| wb.tests.read(cx).summary_text())
    }

    fn coverage_of(wb: &Entity<Workbench>, cx: &mut gpui::VisualTestContext, file: &Path) -> Vec<(usize, bf_core::coverage::LineState)> {
        wb.read_with(cx, |wb, cx| {
            let tab = wb.tabs.iter().find(|t| t.path.as_deref().is_some_and(|p| bf_core::paths::same_path(p, file))).expect("the file is open");
            tab.editor.read(cx).coverage_marks.clone()
        })
    }

    /// A coverage run's reports: the open file's gutter shows them (and a file opened afterwards too), the
    /// Output sums them up without the test projects' files, Show or Hide and Clear take them away.
    #[gpui::test]
    fn coverage_reports_reach_the_gutters_and_the_output(cx: &mut gpui::TestAppContext) {
        use bf_core::coverage::LineState::*;
        let (guard, wb, cx) = workbench(cx, "coverage");
        let ws = guard.dir.join("ws");
        let (lib, tests) = (ws.join("Lib").join("Calc.cs"), ws.join("T").join("CalcTests.cs"));
        std::fs::create_dir_all(lib.parent().unwrap()).unwrap();
        std::fs::create_dir_all(tests.parent().unwrap()).unwrap();
        std::fs::write(&lib, "namespace Lib;\npublic static class Calc\n{\n    public static int Abs(int x)\n    {\n        if (x < 0) return -x;\n        return x;\n    }\n}\n").unwrap();
        std::fs::write(&tests, "public class CalcTests\n{\n    [Fact]\n    public void Abs() => Assert.Equal(3, Lib.Calc.Abs(3));\n}\n").unwrap();
        std::fs::write(ws.join("T").join("T.csproj"), "<Project Sdk=\"Microsoft.NET.Sdk\"><ItemGroup><PackageReference Include=\"Microsoft.NET.Test.Sdk\" Version=\"17.14.1\" /></ItemGroup></Project>").unwrap();
        let report = guard.dir.join("results").join("guid");
        std::fs::create_dir_all(&report).unwrap();
        std::fs::write(
            report.join("run.cobertura.xml"),
            format!(
                "<coverage><packages><package><classes>
                  <class filename=\"{}\"><lines><line number=\"5\" hits=\"1\"/><line number=\"6\" hits=\"1\" branch=\"True\" condition-coverage=\"50% (1/2)\"/><line number=\"7\" hits=\"0\"/></lines></class>
                  <class filename=\"{}\"><lines><line number=\"4\" hits=\"1\"/></lines></class>
                </classes></package></packages></coverage>",
                lib.display(),
                tests.display()
            ),
        )
        .unwrap();
        wb.update_in(cx, |wb, window, cx| {
            wb.open_folder(ws.clone(), window, cx);
            wb.open_file(lib.clone(), None, window, cx);
        });
        cx.dispatch_action(ShowTesting);
        wait(cx, &wb, |wb, cx| wb.tests.read(cx).is_discovered());
        wb.update(cx, |wb, cx| {
            wb.coverage_pending = Some(bf_core::coverage::read_reports(&guard.dir.join("results")));
            wb.finish_coverage(cx);
        });
        cx.run_until_parked();
        assert_eq!(coverage_of(&wb, cx, &lib), vec![(4, Covered), (5, Partial), (6, Uncovered)]);
        let out = wb.read_with(cx, output);
        assert!(out.contains("Coverage: 66.7% of lines (2 of 3), 50.0% of branches (1 of 2)"), "the test project isn't counted: {out}");
        assert!(out.contains("66.7%  Lib") && out.contains("Calc.cs (2 of 3 lines)"), "{out}");

        // A file opened after the run gets its bars too.
        wb.update_in(cx, |wb, window, cx| wb.open_file(tests.clone(), None, window, cx));
        cx.run_until_parked();
        assert_eq!(coverage_of(&wb, cx, &tests), vec![(3, Covered)]);

        // Hide, show again, clear.
        cx.dispatch_action(ToggleCoverage);
        cx.run_until_parked();
        assert!(coverage_of(&wb, cx, &lib).is_empty() && coverage_of(&wb, cx, &tests).is_empty());
        cx.dispatch_action(ToggleCoverage);
        cx.run_until_parked();
        assert_eq!(coverage_of(&wb, cx, &lib).len(), 3);
        cx.dispatch_action(ClearCoverage);
        cx.run_until_parked();
        assert!(coverage_of(&wb, cx, &lib).is_empty());
        cx.dispatch_action(ToggleCoverage);
        assert!(wb.read_with(cx, output).contains("No coverage yet"));
        drop(guard);
    }

    /// Run All Tests with Coverage on a real xunit project testing a library: the library's lines get
    /// covered, partly covered and uncovered bars. Needs the .NET SDK and the xunit template.
    #[gpui::test]
    #[ignore = "runs dotnet test with coverage (about 30 seconds)"]
    fn run_all_tests_with_coverage(cx: &mut gpui::TestAppContext) {
        use bf_core::coverage::LineState::*;
        let (guard, wb, cx) = workbench(cx, "coverage-run");
        let ws = guard.dir.join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        if !dotnet(&ws, &["new", "classlib", "-o", "Lib", "-n", "Lib"]) || !dotnet(&ws, &["new", "xunit", "-o", "T", "-n", "T"]) || !dotnet(&ws, &["add", "T", "reference", "Lib"]) {
            eprintln!("skipped: dotnet or the xunit template is not available");
            return;
        }
        let lib = ws.join("Lib").join("Class1.cs");
        std::fs::write(&lib, "namespace Lib;\n\npublic static class Calc\n{\n    public static int Abs(int x)\n    {\n        if (x < 0)\n        {\n            return -x;\n        }\n        return x;\n    }\n}\n").unwrap();
        std::fs::write(ws.join("T").join("UnitTest1.cs"), "namespace T;\n\npublic class UnitTest1\n{\n    [Fact]\n    public void Positive() => Assert.Equal(3, Lib.Calc.Abs(3));\n}\n").unwrap();
        wb.update_in(cx, |wb, window, cx| {
            wb.open_folder(ws.clone(), window, cx);
            wb.open_file(lib.clone(), None, window, cx);
        });
        cx.dispatch_action(ShowTesting);
        wait(cx, &wb, |wb, cx| wb.tests.read(cx).is_discovered());
        cx.dispatch_action(RunAllTestsWithCoverage);
        wait(cx, &wb, |wb, cx| wb.coverage.is_some() || output(wb, cx).contains("no coverage report"));
        let marks = coverage_of(&wb, cx, &lib);
        assert!(marks.contains(&(6, Partial)), "if (x < 0): one branch taken: {marks:?}\n{}", wb.read_with(cx, output));
        assert!(marks.contains(&(8, Uncovered)), "return -x never ran: {marks:?}");
        assert!(marks.contains(&(10, Covered)), "return x ran: {marks:?}");
        drop(guard);
    }

    /// Run All over two test projects where the first doesn't build: it says so and the second still runs.
    #[gpui::test]
    #[ignore = "runs dotnet test on two projects (about 20 seconds)"]
    fn run_all_goes_on_after_a_project_that_does_not_build(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "run-all-broken");
        let ws = guard.dir.join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        if !dotnet(&ws, &["new", "xunit", "-o", "ABroken", "-n", "ABroken"]) || !dotnet(&ws, &["new", "xunit", "-o", "Good", "-n", "Good"]) {
            eprintln!("skipped: dotnet or the xunit template is not available");
            return;
        }
        std::fs::write(ws.join("ABroken").join("UnitTest1.cs"), "namespace ABroken;\npublic class UnitTest1\n{\n    [Fact]\n    public void Test1() { int x = ; }\n}\n").unwrap();
        wb.update_in(cx, |wb, window, cx| wb.open_folder(ws.clone(), window, cx));
        cx.run_until_parked();
        cx.dispatch_action(ShowTesting);
        wait(cx, &wb, |wb, cx| wb.tests.read(cx).is_discovered());
        let runs: Vec<String> = wb.read_with(cx, |wb, cx| wb.tests.read(cx).run_all().into_iter().map(|r| r.label).collect());
        assert_eq!(runs.len(), 2, "both projects are queued: {runs:?}");
        assert_eq!(runs[0], "ABroken", "the broken one runs first");

        cx.dispatch_action(RunAllTests);
        wait(cx, &wb, |wb, cx| wb.tests.read(cx).test_state("Good.UnitTest1.Test1") == TestState::Passed);
        let out = wb.read_with(cx, output);
        assert!(out.contains("no results for ABroken"), "{out}");
        drop(guard);
    }

    /// The Testing view while a real `dotnet test` runs: tests turn running, then passed or failed one by
    /// one, the counts add up at the end, Rerun Failed runs only the failure, and Run Test at Cursor the
    /// test the caret is in. Needs the .NET SDK and the xunit template (run with --ignored).
    #[gpui::test]
    #[ignore = "runs dotnet test (about 20 seconds)"]
    fn live_test_progress_in_the_testing_view(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "live-tests");
        let ws = guard.dir.join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        if !dotnet(&ws, &["new", "xunit", "-o", "XT", "-n", "XT"]) {
            eprintln!("skipped: dotnet or the xunit template is not available");
            return;
        }
        let file = ws.join("XT").join("UnitTest1.cs");
        std::fs::write(&file, TESTS).unwrap();

        wb.update_in(cx, |wb, window, cx| wb.open_folder(ws.clone(), window, cx));
        cx.run_until_parked();
        cx.dispatch_action(ShowTesting);
        wait(cx, &wb, |wb, cx| wb.tests.read(cx).is_discovered());
        assert_eq!(summary(cx, &wb), "0 passed · 4 not run");

        // Run All: everything is running at first, then the fast class finishes while Slow still runs.
        click(cx, "tests-run-all");
        assert!(wb.read_with(cx, |wb, cx| ["CalcTests.Adds", "SlowTests.Slow"].iter().all(|t| state(wb, cx, t) == TestState::Running)));
        wait(cx, &wb, |wb, cx| ["CalcTests.Adds", "CalcTests.Fails", "CalcTests.Positive"].iter().all(|t| state(wb, cx, t) != TestState::Running));
        wb.read_with(cx, |wb, cx| {
            assert_eq!(state(wb, cx, "CalcTests.Fails"), TestState::Failed);
            assert_eq!(state(wb, cx, "CalcTests.Adds"), TestState::Passed);
            assert_eq!(state(wb, cx, "SlowTests.Slow"), TestState::Running, "{}", output(wb, cx));
        });
        assert_eq!(summary(cx, &wb), "Running 1 test… 1 failed so far");
        wait(cx, &wb, |wb, cx| !wb.tests.read(cx).is_running());
        wb.read_with(cx, |wb, cx| {
            assert_eq!(state(wb, cx, "CalcTests.Adds"), TestState::Passed);
            assert_eq!(state(wb, cx, "CalcTests.Positive"), TestState::Passed);
            assert_eq!(state(wb, cx, "SlowTests.Slow"), TestState::Passed);
        });
        assert_eq!(summary(cx, &wb), "3 passed · 1 failed");

        // Rerun Failed (the button shows once something failed) runs only Fails.
        click(cx, "tests-run-failed");
        wb.read_with(cx, |wb, cx| {
            assert_eq!(state(wb, cx, "CalcTests.Fails"), TestState::Running, "{}", output(wb, cx));
            assert_eq!(state(wb, cx, "SlowTests.Slow"), TestState::Passed);
        });
        wait(cx, &wb, |wb, cx| !wb.tests.read(cx).is_running());
        assert_eq!(summary(cx, &wb), "3 passed · 1 failed");

        // Fix the test, put the caret inside it, Run Test at Cursor.
        std::fs::write(&file, TESTS.replace("Assert.Equal(5, 2 + 2)", "Assert.Equal(4, 2 + 2)")).unwrap();
        wb.update_in(cx, |wb, window, cx| wb.open_file(file.clone(), Some(Point::new(8, 20)), window, cx));
        cx.run_until_parked();
        cx.dispatch_action(RunTestAtCursor);
        cx.run_until_parked();
        wb.read_with(cx, |wb, cx| {
            assert_eq!(state(wb, cx, "CalcTests.Fails"), TestState::Running);
            assert_eq!(state(wb, cx, "CalcTests.Adds"), TestState::Passed);
        });
        wait(cx, &wb, |wb, cx| !wb.tests.read(cx).is_running());
        assert_eq!(summary(cx, &wb), "4 passed");
        drop(guard);
    }
}
