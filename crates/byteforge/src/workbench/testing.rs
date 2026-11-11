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
