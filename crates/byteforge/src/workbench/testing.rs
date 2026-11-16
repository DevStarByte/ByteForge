//! Tests in the workbench: running and debugging them with `dotnet test`, reading the TRX results back
//! into the Test Explorer, and the pass/fail markers in the editors' gutters.

use bf_core::testing::{self as tst, Outcome, TestResult};

use super::*;
use crate::testing::{TestEvent, TestRun, TestView};

impl Workbench {
    pub(super) fn on_test_event(&mut self, _: &Entity<TestView>, event: &TestEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            TestEvent::Run(runs) => self.run_tests(runs.clone(), false, window, cx),
            TestEvent::Debug(run) => self.run_tests(vec![run.clone()], true, window, cx),
            TestEvent::OpenFile(path, line) => self.open_file(path.clone(), Some(Point::new(*line, 0)), window, cx),
            TestEvent::Changed => self.refresh_test_marks(cx),
        }
    }

    /// Runs the queue one `dotnet test` after another. With `debug`, the test host waits for the debugger
    /// (`VSTEST_HOST_DEBUG=1`) and is attached to as soon as it prints its process id.
    pub(super) fn run_tests(&mut self, mut queue: Vec<TestRun>, debug: bool, window: &mut Window, cx: &mut Context<Self>) {
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
        self.tests.update(cx, |v, cx| v.start_run(&run, cx));
        self.show_panel(PanelView::Output, cx);
        self.log(format!("{} tests: {}", if debug { "Debugging" } else { "Running" }, run.label));

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
            let _ = std::fs::remove_dir_all(&results_dir);
            if results.is_empty() {
                wb.log(format!("Tests: no results for {label} (did the build fail? see above)."));
                wb.tests.update(cx, |v, cx| v.cancel_run(cx));
                return;
            }
            wb.log(summary_line(&label, &results));
            wb.tests.update(cx, |v, cx| v.finish_run(results, cx));
            if !queue.is_empty() {
                wb.run_tests(queue, false, window, cx);
            }
        });
        if !self.start_dotnet_with(root, args, env, on_line, Some(then), window, cx) {
            self.tests.update(cx, |v, cx| v.cancel_run(cx));
        }
    }

    pub(super) fn run_all_tests(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_tests_discovered(cx);
        let runs = self.tests.read(cx).run_all();
        if runs.is_empty() {
            // Not discovered yet, or no test project: fall back to plain `dotnet test`.
            self.run_dotnet("test", window, cx);
            return;
        }
        self.run_tests(runs, false, window, cx);
    }

    pub(super) fn rerun_failed_tests(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let runs = self.tests.read(cx).run_failed();
        if runs.is_empty() {
            self.log("No failed tests to rerun.");
            self.show_panel(PanelView::Output, cx);
            return;
        }
        self.run_tests(runs, false, window, cx);
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
            Some(run) => self.run_tests(vec![run], debug, window, cx),
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
        panic!("timed out; output:\n{}", wb.read_with(cx, |wb, cx| output(wb, cx)));
    }

    fn state(wb: &Workbench, cx: &App, test: &str) -> TestState {
        wb.tests.read(cx).test_state(&format!("XT.{test}"))
    }

    fn summary(cx: &mut gpui::VisualTestContext, wb: &Entity<Workbench>) -> String {
        wb.read_with(cx, |wb, cx| wb.tests.read(cx).summary_text())
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
