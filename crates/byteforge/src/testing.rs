//! Test Explorer: the workspace's tests (project → class → test) with their last results. Running and
//! debugging go through the workbench (`dotnet test`), which reports the results back here.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use bf_core::Workspace;
use bf_core::testing::{self, Outcome, TestProject, TestResult};
use gpui::{Context, EventEmitter, Hsla, Task, Window, div, prelude::*, px, uniform_list};

use crate::assets::icon;
use crate::theme::Theme;
use crate::workbench::tooltip;

/// One `dotnet test` invocation: a project, optionally narrowed by a `--filter`.
#[derive(Clone, Debug)]
pub struct TestRun {
    pub project: PathBuf,
    pub filter: Option<String>,
    /// Shown in the Output panel and the status line ("CalculatorTests.Adds", "XTests", …).
    pub label: String,
}

pub enum TestEvent {
    /// Run these one after another (Run All runs every test project).
    Run(Vec<TestRun>),
    Debug(TestRun),
    OpenFile(PathBuf, usize),
    /// Discovery or results changed: the gutter markers need updating.
    Changed,
}

/// What a test (or a group, aggregated) last did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestState {
    NotRun,
    Running,
    Passed,
    Failed,
    Skipped,
}

impl TestState {
    fn combine(self, other: TestState) -> TestState {
        use TestState::*;
        match (self, other) {
            (Running, _) | (_, Running) => Running,
            (Failed, _) | (_, Failed) => Failed,
            (NotRun, _) | (_, NotRun) => NotRun,
            (Passed, _) | (_, Passed) => Passed,
            _ => Skipped,
        }
    }
}

/// The aggregated result of one test method (a theory has several cases).
#[derive(Clone, Debug)]
struct Summary {
    outcome: Outcome,
    seconds: f64,
    cases: usize,
    failed_cases: usize,
    /// The first failure: display name, message and stack trace.
    failure: Option<(String, String, String)>,
    /// Each case of a theory (`[InlineData]`, `[MemberData]`, …), in the order they ran.
    case_list: Vec<CaseResult>,
}

/// One case of a test with several (its display name carries the arguments).
#[derive(Clone, Debug)]
struct CaseResult {
    display: String,
    outcome: Outcome,
    seconds: f64,
    message: Option<String>,
}

impl Summary {
    fn new() -> Self {
        Summary { outcome: Outcome::Passed, seconds: 0.0, cases: 0, failed_cases: 0, failure: None, case_list: Vec::new() }
    }

    /// Counts one result (a case) into the summary.
    fn add(&mut self, r: &TestResult) {
        self.cases += 1;
        self.seconds += r.seconds;
        self.case_list.push(CaseResult { display: r.display_name.clone(), outcome: r.outcome, seconds: r.seconds, message: r.message.clone() });
        match r.outcome {
            Outcome::Failed => {
                self.failed_cases += 1;
                self.outcome = Outcome::Failed;
                if self.failure.is_none() {
                    self.failure = Some((r.display_name.clone(), r.message.clone().unwrap_or_default(), r.stack_trace.clone().unwrap_or_default()));
                }
            }
            Outcome::Skipped if self.cases == 1 => self.outcome = Outcome::Skipped,
            Outcome::Passed if self.outcome == Outcome::Skipped => self.outcome = Outcome::Passed,
            _ => {}
        }
    }
}

#[derive(Clone, Copy)]
enum Entry {
    Project(usize),
    Class(usize, usize),
    Test(usize, usize, usize),
    /// Case `k` of a test with several.
    Case(usize, usize, usize, usize),
}

pub struct TestView {
    workspace: Workspace,
    projects: Vec<TestProject>,
    results: HashMap<String, Summary>,
    /// Results of the run in progress, as the tests report them (replaced by the TRX results at the end).
    live: HashMap<String, Summary>,
    /// Tests whose cases are listed.
    expanded: HashSet<String>,
    /// Full names (tests) currently running; a project or class run marks all its tests.
    running: HashSet<String>,
    collapsed: HashSet<String>,
    entries: Vec<Entry>,
    discovered: bool,
    visible: bool,
    discover_task: Option<Task<()>>,
}

impl EventEmitter<TestEvent> for TestView {}

impl TestView {
    pub fn new(workspace: Workspace, _: &mut Context<Self>) -> Self {
        Self {
            workspace,
            projects: Vec::new(),
            results: HashMap::new(),
            live: HashMap::new(),
            expanded: HashSet::new(),
            running: HashSet::new(),
            collapsed: HashSet::new(),
            entries: Vec::new(),
            discovered: false,
            visible: false,
            discover_task: None,
        }
    }

    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if visible && !self.visible && !self.discovered {
            self.discover(cx);
        }
        self.visible = visible;
    }

    /// Files changed (or a folder was opened): find the tests again.
    pub fn files_changed(&mut self, cx: &mut Context<Self>) {
        self.discover(cx);
    }

    pub fn open_folder(&mut self, cx: &mut Context<Self>) {
        self.projects.clear();
        self.results.clear();
        self.running.clear();
        self.discovered = false;
        self.rebuild();
        self.discover(cx);
    }

    pub fn discover(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.root() else { return };
        self.discover_task = Some(cx.spawn(async move |this, cx| {
            let projects = cx.background_executor().spawn(async move { testing::discover(&root) }).await;
            let _ = this.update(cx, |v, cx| {
                v.projects = projects;
                v.discovered = true;
                v.rebuild();
                cx.emit(TestEvent::Changed);
                cx.notify();
            });
        }));
    }

    pub fn is_discovered(&self) -> bool {
        self.discovered
    }

    /// Discovery results computed elsewhere (a command ran before the view was ever shown).
    pub fn set_projects(&mut self, projects: Vec<TestProject>, cx: &mut Context<Self>) {
        self.projects = projects;
        self.discovered = true;
        self.rebuild();
        cx.emit(TestEvent::Changed);
        cx.notify();
    }

    pub fn is_running(&self) -> bool {
        !self.running.is_empty()
    }

    fn rebuild(&mut self) {
        let mut entries = Vec::new();
        for (pi, project) in self.projects.iter().enumerate() {
            entries.push(Entry::Project(pi));
            if self.collapsed.contains(&project_key(&project.path)) {
                continue;
            }
            for (ci, class) in project.classes.iter().enumerate() {
                entries.push(Entry::Class(pi, ci));
                if self.collapsed.contains(&class.full_name) {
                    continue;
                }
                for (ti, test) in class.tests.iter().enumerate() {
                    entries.push(Entry::Test(pi, ci, ti));
                    let name = test.full_name();
                    if self.expanded.contains(&name)
                        && let Some(summary) = self.summary(&name).filter(|s| s.case_list.len() > 1)
                    {
                        entries.extend((0..summary.case_list.len()).map(|k| Entry::Case(pi, ci, ti, k)));
                    }
                }
            }
        }
        self.entries = entries;
    }

    fn toggle(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.collapsed.remove(&key) {
            self.collapsed.insert(key);
        }
        self.rebuild();
        cx.notify();
    }

    fn toggle_cases(&mut self, name: String, cx: &mut Context<Self>) {
        if !self.expanded.remove(&name) {
            self.expanded.insert(name);
        }
        self.rebuild();
        cx.notify();
    }

    // ───────────── state ─────────────

    /// The latest result of a test: from the run in progress, else the last run.
    fn summary(&self, full_name: &str) -> Option<&Summary> {
        self.live.get(full_name).or_else(|| self.results.get(full_name))
    }

    fn test_state(&self, full_name: &str) -> TestState {
        if self.running.contains(full_name) {
            return TestState::Running;
        }
        match self.summary(full_name).map(|s| s.outcome) {
            None => TestState::NotRun,
            Some(Outcome::Passed) => TestState::Passed,
            Some(Outcome::Failed) => TestState::Failed,
            Some(Outcome::Skipped) => TestState::Skipped,
        }
    }

    fn group_state<'a>(&self, names: impl Iterator<Item = &'a String>) -> TestState {
        names.map(|n| self.test_state(n)).reduce(TestState::combine).unwrap_or(TestState::NotRun)
    }

    fn class_state(&self, pi: usize, ci: usize) -> TestState {
        let names: Vec<String> = self.projects[pi].classes[ci].tests.iter().map(|t| t.full_name()).collect();
        self.group_state(names.iter())
    }

    fn project_state(&self, pi: usize) -> TestState {
        let names: Vec<String> = self.projects[pi].classes.iter().flat_map(|c| c.tests.iter().map(|t| t.full_name())).collect();
        self.group_state(names.iter())
    }

    /// (passed, failed, skipped, not run) over all tests.
    fn counts(&self) -> (usize, usize, usize, usize) {
        let mut c = (0, 0, 0, 0);
        for test in self.projects.iter().flat_map(|p| p.classes.iter().flat_map(|c| c.tests.iter())) {
            match self.test_state(&test.full_name()) {
                TestState::Passed => c.0 += 1,
                TestState::Failed => c.1 += 1,
                TestState::Skipped => c.2 += 1,
                TestState::NotRun | TestState::Running => c.3 += 1,
            }
        }
        c
    }

    /// Gutter markers for a file: each test method and each test class with its state.
    pub fn marks_for(&self, path: &Path) -> Vec<(usize, TestState)> {
        let mut marks = Vec::new();
        for (pi, project) in self.projects.iter().enumerate() {
            for (ci, class) in project.classes.iter().enumerate() {
                if bf_core::paths::same_path(&class.file, path) {
                    marks.push((class.line, self.class_state(pi, ci)));
                }
                for test in &class.tests {
                    if bf_core::paths::same_path(&test.file, path) {
                        marks.push((test.line, self.test_state(&test.full_name())));
                    }
                }
            }
        }
        marks.sort_by_key(|m| m.0);
        marks
    }

    /// The test (or else class) whose declaration is at or above `line` in `path`, closest first:
    /// what "Run Test at Cursor" runs.
    pub fn run_at(&self, path: &Path, line: usize) -> Option<TestRun> {
        let mut best: Option<(usize, TestRun)> = None;
        for project in &self.projects {
            for class in &project.classes {
                let candidates = class
                    .tests
                    .iter()
                    .filter(|t| bf_core::paths::same_path(&t.file, path))
                    .map(|t| (t.line, test_run(&project.path, &t.full_name(), false, short(&t.class, &t.name))))
                    .chain(bf_core::paths::same_path(&class.file, path).then(|| (class.line, test_run(&project.path, &class.full_name, true, class.short_name().to_owned()))));
                for (l, run) in candidates {
                    if l <= line && best.as_ref().is_none_or(|(b, _)| l > *b) {
                        best = Some((l, run));
                    }
                }
            }
        }
        best.map(|(_, run)| run)
    }

    /// One run per test project.
    pub fn run_all(&self) -> Vec<TestRun> {
        self.projects.iter().map(|p| TestRun { project: p.path.clone(), filter: None, label: p.name.clone() }).collect()
    }

    /// The failed tests, one run per project (filters joined with `|`).
    pub fn run_failed(&self) -> Vec<TestRun> {
        self.projects
            .iter()
            .filter_map(|p| {
                let failed: Vec<String> = p
                    .classes
                    .iter()
                    .flat_map(|c| c.tests.iter().map(|t| t.full_name()))
                    .filter(|n| self.test_state(n) == TestState::Failed)
                    .collect();
                (!failed.is_empty()).then(|| TestRun {
                    project: p.path.clone(),
                    filter: Some(failed.iter().map(|n| testing::filter(n, false)).collect::<Vec<_>>().join("|")),
                    label: format!("{} failed in {}", failed.len(), p.name),
                })
            })
            .collect()
    }

    /// Marks the tests a run covers as running.
    pub fn start_run(&mut self, run: &TestRun, cx: &mut Context<Self>) {
        for project in self.projects.iter().filter(|p| p.path == run.project) {
            for test in project.classes.iter().flat_map(|c| c.tests.iter()) {
                let name = test.full_name();
                if covers(run.filter.as_deref(), &name) {
                    self.running.insert(name);
                }
            }
        }
        cx.emit(TestEvent::Changed);
        cx.notify();
    }

    /// A test (or one of its cases) just finished, while the run goes on: `display` as the console logger
    /// printed it. Matched to a running test by its full name, or (NUnit's short names) by its method name.
    pub fn live_result(&mut self, outcome: Outcome, display: &str, cx: &mut Context<Self>) {
        let base = display.split('(').next().unwrap_or(display).trim();
        let name = if self.running.contains(base) || self.live.contains_key(base) {
            base.to_owned()
        } else {
            let suffix = format!(".{base}");
            let mut candidates = self.running.iter().chain(self.live.keys()).filter(|n| n.ends_with(&suffix));
            match (candidates.next(), candidates.next()) {
                (Some(only), None) => only.clone(),
                _ => return,
            }
        };
        let result = TestResult { full_name: name.clone(), display_name: display.to_owned(), outcome, seconds: 0.0, message: None, stack_trace: None };
        self.live.entry(name.clone()).or_insert_with(Summary::new).add(&result);
        self.running.remove(&name);
        self.rebuild();
        cx.emit(TestEvent::Changed);
        cx.notify();
    }

    /// Applies a run's results; tests the run covered but didn't report keep their old result.
    pub fn finish_run(&mut self, results: Vec<TestResult>, cx: &mut Context<Self>) {
        let mut fresh: HashMap<String, Summary> = HashMap::new();
        for r in results {
            fresh.entry(r.full_name.clone()).or_insert_with(Summary::new).add(&r);
        }
        self.results.extend(fresh);
        self.live.clear();
        self.running.clear();
        self.rebuild();
        cx.emit(TestEvent::Changed);
        cx.notify();
    }

    /// The run ended without results (build failed, cancelled): forget the running marks.
    pub fn cancel_run(&mut self, cx: &mut Context<Self>) {
        self.running.clear();
        self.live.clear();
        self.rebuild();
        cx.emit(TestEvent::Changed);
        cx.notify();
    }
}

fn project_key(path: &Path) -> String {
    format!("project:{}", path.display())
}

fn short(class: &str, method: &str) -> String {
    let head = class.split('+').next().unwrap_or(class);
    let ns_len = head.rfind('.').map_or(0, |i| i + 1);
    format!("{}.{method}", &class[ns_len..])
}

fn test_run(project: &Path, full_name: &str, is_class: bool, label: String) -> TestRun {
    TestRun { project: project.to_path_buf(), filter: Some(testing::filter(full_name, is_class)), label }
}

/// Whether a run with this filter (as built by [`testing::filter`]) includes the test.
fn covers(filter: Option<&str>, full_name: &str) -> bool {
    let Some(filter) = filter else { return true };
    filter.split('|').any(|part| {
        let unescape = |s: &str| s.replace('\\', "");
        if let Some(exact) = part.strip_prefix("FullyQualifiedName=") {
            unescape(exact) == full_name
        } else if let Some(prefix) = part.strip_prefix("FullyQualifiedName~") {
            full_name.starts_with(&unescape(prefix))
        } else {
            true
        }
    })
}

pub fn state_color(state: TestState, t: &Theme) -> Hsla {
    match state {
        TestState::Passed => t.c_or("testing.iconPassed", 0x73c991ff),
        TestState::Failed => t.c_or("testing.iconFailed", 0xf14c4cff),
        TestState::Running => t.c_or("testing.iconQueued", 0xcca700ff),
        TestState::Skipped => t.c_or("testing.iconSkipped", 0x848484ff),
        TestState::NotRun => t.c_or("testing.iconUnset", 0x848484ff),
    }
}

fn state_icon(state: TestState) -> &'static str {
    match state {
        TestState::Passed => "check",
        TestState::Failed => "close",
        TestState::Running => "refresh",
        TestState::Skipped => "remove",
        TestState::NotRun => "breakpoint",
    }
}

impl Render for TestView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::global(cx);
        let muted = t.c("foreground.muted");
        let icon_size = t.m("scm.iconSize", 14.0);
        let tool = |id: &'static str, name: &'static str, tip: &'static str| {
            div()
                .id(id)
                .p(px(3.))
                .rounded(px(4.))
                .cursor_pointer()
                .hover(|s| s.bg(t.c("toolbar.hoverBackground")))
                .tooltip(tooltip(tip))
                .child(icon(name, icon_size, t.c("icon.foreground")))
        };
        let (passed, failed, skipped, not_run) = self.counts();
        let summary = if self.workspace.root().is_none() {
            "Open a folder with a .NET test project.".to_owned()
        } else if !self.discovered {
            "Looking for tests…".to_owned()
        } else if self.projects.is_empty() {
            "No tests found. Test projects reference Microsoft.NET.Test.Sdk (xUnit, NUnit or MSTest).".to_owned()
        } else if self.is_running() {
            format!("Running {} tests…", self.running.len())
        } else {
            let mut parts = vec![format!("{passed} passed")];
            if failed > 0 {
                parts.push(format!("{failed} failed"));
            }
            if skipped > 0 {
                parts.push(format!("{skipped} skipped"));
            }
            if not_run > 0 {
                parts.push(format!("{not_run} not run"));
            }
            parts.join(" · ")
        };

        let entity = cx.entity();
        let row_h = t.n("list.itemHeight", 22.0);
        let list = uniform_list("test-entries", self.entries.len(), move |range, _, cx| {
            let view = entity.read(cx);
            let t = Theme::global(cx);
            range
                .map(|ix| {
                    let entry = view.entries[ix];
                    let small = |id: &'static str, name: &'static str, tip: &'static str| {
                        div()
                            .id(id)
                            .p(px(2.))
                            .rounded(px(3.))
                            .cursor_pointer()
                            .hover(|s| s.bg(t.c("toolbar.hoverBackground")))
                            .tooltip(tooltip(tip))
                            .child(icon(name, px(14.), t.c("icon.foreground")))
                    };
                    let row = |indent: f32| {
                        div()
                            .id(ix)
                            .group("test-row")
                            .w_full()
                            .h(px(row_h))
                            .pl(px(indent))
                            .pr_3()
                            .flex()
                            .items_center()
                            .gap(t.m("explorer.itemSpacing", 6.0))
                            .text_size(t.m("list.fontSize", 13.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(t.c("list.hoverBackground")))
                    };
                    let detail = |text: String| div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_size(px(11.)).text_color(t.c("foreground.muted")).child(text);
                    let chevron = |collapsed: bool| icon(if collapsed { "chevronRight" } else { "chevronDown" }, px(14.), t.c("icon.foreground"));
                    let buttons = |run: TestRun| {
                        let (e1, e2) = (entity.clone(), entity.clone());
                        let (r1, r2) = (run.clone(), run);
                        div()
                            .flex()
                            .gap_1()
                            .invisible()
                            .group_hover("test-row", |s| s.visible())
                            .child(small("run", "play", "Run").on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                e1.update(cx, |_, cx| cx.emit(TestEvent::Run(vec![r1.clone()])))
                            }))
                            .child(small("debug", "debug", "Debug").on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                e2.update(cx, |_, cx| cx.emit(TestEvent::Debug(r2.clone())))
                            }))
                    };
                    match entry {
                        Entry::Project(pi) => {
                            let project = &view.projects[pi];
                            let key = project_key(&project.path);
                            let collapsed = view.collapsed.contains(&key);
                            let state = view.project_state(pi);
                            let e = entity.clone();
                            row(6.)
                                .child(chevron(collapsed))
                                .child(icon(state_icon(state), px(12.), state_color(state, &t)))
                                .child(div().whitespace_nowrap().font_weight(gpui::FontWeight::SEMIBOLD).child(project.name.clone()))
                                .child(detail(format!("{} tests", project.test_count())))
                                .child(buttons(TestRun { project: project.path.clone(), filter: None, label: project.name.clone() }))
                                .on_click(move |_, _, cx| e.update(cx, |v, cx| v.toggle(key.clone(), cx)))
                                .into_any_element()
                        }
                        Entry::Class(pi, ci) => {
                            let project = &view.projects[pi];
                            let class = &project.classes[ci];
                            let collapsed = view.collapsed.contains(&class.full_name);
                            let state = view.class_state(pi, ci);
                            let (e, key) = (entity.clone(), class.full_name.clone());
                            let ns = class.full_name.strip_suffix(class.short_name()).unwrap_or_default().trim_end_matches('.').to_owned();
                            row(20.)
                                .tooltip(tooltip(class.full_name.clone()))
                                .child(chevron(collapsed))
                                .child(icon(state_icon(state), px(12.), state_color(state, &t)))
                                .child(div().whitespace_nowrap().child(class.short_name().to_owned()))
                                .child(detail(ns))
                                .child(buttons(test_run(&project.path, &class.full_name, true, class.short_name().to_owned())))
                                .on_click(move |_, _, cx| e.update(cx, |v, cx| v.toggle(key.clone(), cx)))
                                .into_any_element()
                        }
                        Entry::Test(pi, ci, ti) => {
                            let project = &view.projects[pi];
                            let test = &project.classes[ci].tests[ti];
                            let name = test.full_name();
                            let state = view.test_state(&name);
                            let summary = view.summary(&name);
                            let detail_text = match summary {
                                Some(s) if s.cases > 1 && s.failed_cases > 0 => format!("{}/{} cases failed", s.failed_cases, s.cases),
                                Some(s) if s.cases > 1 => format!("{} cases · {}", s.cases, duration(s.seconds)),
                                Some(s) => duration(s.seconds),
                                None => String::new(),
                            };
                            let mut tip = name.clone();
                            if let Some((display, message, stack)) = summary.and_then(|s| s.failure.as_ref()) {
                                tip = format!("{display}\n\n{message}");
                                if !stack.is_empty() {
                                    tip.push_str(&format!("\n\n{}", stack.lines().take(6).collect::<Vec<_>>().join("\n")));
                                }
                            }
                            let (e, file, line) = (entity.clone(), test.file.clone(), test.line);
                            // A theory's cases fold out under it.
                            let has_cases = summary.is_some_and(|s| s.case_list.len() > 1);
                            let (toggle, toggle_name) = (entity.clone(), name.clone());
                            row(if has_cases { 22. } else { 38. })
                                .tooltip(tooltip(tip))
                                .when(has_cases, |d| {
                                    d.child(div().id(gpui::SharedString::from(format!("cases-{ix}"))).child(chevron(!view.expanded.contains(&name))).on_click(move |_, _, cx| {
                                        cx.stop_propagation();
                                        toggle.update(cx, |v, cx| v.toggle_cases(toggle_name.clone(), cx));
                                    }))
                                })
                                .child(icon(state_icon(state), px(12.), state_color(state, &t)))
                                .child(div().whitespace_nowrap().child(test.name.clone()))
                                .child(detail(detail_text))
                                .child(buttons(test_run(&project.path, &name, false, short(&test.class, &test.name))))
                                .on_click(move |_, _, cx| e.update(cx, |_, cx| cx.emit(TestEvent::OpenFile(file.clone(), line))))
                                .into_any_element()
                        }
                        Entry::Case(pi, ci, ti, k) => {
                            let test = &view.projects[pi].classes[ci].tests[ti];
                            let Some(case) = view.summary(&test.full_name()).and_then(|s| s.case_list.get(k)) else { return div().id(ix).into_any_element() };
                            let state = match case.outcome {
                                Outcome::Passed => TestState::Passed,
                                Outcome::Failed => TestState::Failed,
                                Outcome::Skipped => TestState::Skipped,
                            };
                            // The arguments are what tells cases apart: show them after the method name.
                            let args = case.display.find('(').map_or(case.display.as_str(), |at| &case.display[at..]).to_owned();
                            let tip = match &case.message {
                                Some(m) => format!("{}\n\n{m}", case.display),
                                None => case.display.clone(),
                            };
                            let (e, file, line) = (entity.clone(), test.file.clone(), test.line);
                            row(56.)
                                .tooltip(tooltip(tip))
                                .child(icon(state_icon(state), px(12.), state_color(state, &t)))
                                .child(div().whitespace_nowrap().child(format!("{}{args}", test.name)))
                                .child(detail(if case.seconds > 0.0 { duration(case.seconds) } else { String::new() }))
                                .on_click(move |_, _, cx| e.update(cx, |_, cx| cx.emit(TestEvent::OpenFile(file.clone(), line))))
                                .into_any_element()
                        }
                    }
                })
                .collect()
        })
        .flex_1();

        let running = self.is_running();
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(icon("beaker", icon_size, if failed > 0 { state_color(TestState::Failed, &t) } else { t.c("icon.foreground") }))
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_size(px(12.)).text_color(muted).child(summary))
                    .when(!running, |d| {
                        d.child(tool("tests-run-all", "play", "Run All Tests").on_click(cx.listener(|v, _, _, cx| {
                            let runs = v.run_all();
                            if !runs.is_empty() {
                                cx.emit(TestEvent::Run(runs));
                            }
                        })))
                        .when(failed > 0, |d| {
                            d.child(tool("tests-run-failed", "refresh", "Rerun Failed Tests").on_click(cx.listener(|v, _, _, cx| {
                                let runs = v.run_failed();
                                if !runs.is_empty() {
                                    cx.emit(TestEvent::Run(runs));
                                }
                            })))
                        })
                    })
                    .child(tool("tests-refresh", "search", "Find Tests Again").on_click(cx.listener(|v, _, _, cx| v.discover(cx)))),
            )
            .child(list)
            .into_any_element()
    }
}

fn duration(seconds: f64) -> String {
    if seconds < 1.0 { format!("{:.0} ms", seconds * 1000.0) } else { format!("{seconds:.1} s") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_cover_their_tests() {
        let one = testing::filter("A.T.Adds", false);
        assert!(covers(Some(&one), "A.T.Adds"));
        assert!(!covers(Some(&one), "A.T.Adds2"));
        let class = testing::filter("A.T", true);
        assert!(covers(Some(&class), "A.T.Adds"));
        assert!(!covers(Some(&class), "A.TX.Adds"));
        let both = format!("{}|{}", testing::filter("A.T.X", false), testing::filter("A.T.Y", false));
        assert!(covers(Some(&both), "A.T.Y"));
        assert!(covers(None, "anything"));
        assert_eq!(short("Ns.Outer+Inner", "M"), "Outer+Inner.M");
    }
}

#[cfg(test)]
mod view_tests {
    use super::*;
    use bf_core::testing::{TestCase, TestClass};
    use gpui::AppContext as _;

    fn project() -> TestProject {
        let file = PathBuf::from("/w/T/MathTests.cs");
        let test = |name: &str, line| TestCase { name: name.into(), class: "Calc.MathTests".into(), file: file.clone(), line };
        TestProject {
            path: PathBuf::from("/w/T/T.csproj"),
            name: "T".into(),
            classes: vec![TestClass { full_name: "Calc.MathTests".into(), file: file.clone(), line: 2, tests: vec![test("Adds", 5), test("Positive", 9)] }],
        }
    }

    fn result(name: &str, display: &str, outcome: Outcome) -> TestResult {
        TestResult { full_name: name.into(), display_name: display.into(), outcome, seconds: 0.01, message: None, stack_trace: None }
    }

    #[gpui::test]
    fn live_progress_then_theory_cases(cx: &mut gpui::TestAppContext) {
        let view = cx.new(|cx| TestView::new(Workspace::new(Vec::new()), cx));
        view.update(cx, |v, cx| {
            v.set_projects(vec![project()], cx);
            v.start_run(&TestRun { project: PathBuf::from("/w/T/T.csproj"), filter: None, label: "T".into() }, cx);
            assert_eq!(v.test_state("Calc.MathTests.Adds"), TestState::Running);
            // The console logger reports a test: it shows at once, the other still runs.
            v.live_result(Outcome::Failed, "Calc.MathTests.Adds", cx);
            assert_eq!(v.test_state("Calc.MathTests.Adds"), TestState::Failed);
            assert_eq!(v.test_state("Calc.MathTests.Positive"), TestState::Running);
            // NUnit-style short names are matched by method name.
            v.live_result(Outcome::Passed, "Positive(1)", cx);
            assert_eq!(v.test_state("Calc.MathTests.Positive"), TestState::Passed);

            v.finish_run(
                vec![
                    result("Calc.MathTests.Adds", "Calc.MathTests.Adds", Outcome::Passed),
                    result("Calc.MathTests.Positive", "Calc.MathTests.Positive(x: 1)", Outcome::Passed),
                    result("Calc.MathTests.Positive", "Calc.MathTests.Positive(x: -1)", Outcome::Failed),
                ],
                cx,
            );
            // The TRX results replace the live ones.
            assert_eq!(v.test_state("Calc.MathTests.Adds"), TestState::Passed);
            assert_eq!(v.test_state("Calc.MathTests.Positive"), TestState::Failed);
            let cases = |v: &TestView| v.entries.iter().filter(|e| matches!(e, Entry::Case(..))).count();
            assert_eq!(cases(v), 0, "cases start folded");
            v.toggle_cases("Calc.MathTests.Positive".into(), cx);
            assert_eq!(cases(v), 2);
        });
    }
}
