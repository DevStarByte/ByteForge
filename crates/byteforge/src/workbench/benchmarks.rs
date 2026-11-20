//! Benchmarks: "Run Benchmark" (Code Lens, right-click) runs a BenchmarkDotNet benchmark in Release in a
//! terminal, and the Benchmarks panel tab shows the results its runs leave behind
//! (`BenchmarkDotNet.Artifacts/results/*-report-full.json`): mean, error, allocated, and the change since
//! the run before.

use std::time::SystemTime;

use bf_core::benchmarks::{self as bench, BenchmarkResult, History};

use super::*;

/// Where the measurements of the last runs are kept between sessions (in the state file).
const HISTORY_KEY: &str = "benchmarks.history";

#[derive(Default)]
pub(super) struct BenchmarksState {
    /// The reports found, newest first: (report file, written, results).
    reports: Vec<(PathBuf, SystemTime, Vec<BenchmarkResult>)>,
    history: Option<History>,
    loading: Option<Task<()>>,
    /// A run was started here: show the tab when its results arrive.
    expecting: bool,
}

impl BenchmarksState {
    pub(super) fn has_results(&self) -> bool {
        !self.reports.is_empty()
    }

    /// Whether files changing on disk may have brought new results worth reading.
    pub(super) fn watching(&self) -> bool {
        self.expecting || self.has_results()
    }
}

impl Workbench {
    /// "Run Benchmark" on `line` of `path` (a lens, or the caret for "Run Benchmark at Cursor").
    pub(super) fn run_benchmark_at(&mut self, path: &Path, line: usize, window: &mut Window, cx: &mut Context<Self>) {
        let text = match self.tabs.iter().find(|t| t.path.as_deref().is_some_and(|p| bf_core::paths::same_path(p, path))) {
            Some(tab) => tab.editor.read(cx).state.buffer.text(),
            None => std::fs::read_to_string(path).unwrap_or_default(),
        };
        let Some(run) = bench::run_at(&bench::scan_source(&text), line) else {
            self.log("Run Benchmark: there's no [Benchmark] method or benchmark class here.");
            self.show_panel(PanelView::Output, cx);
            return;
        };
        let Some(project) = path.ancestors().skip(1).find_map(|dir| {
            std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("csproj")))
        }) else {
            self.log(format!("Run Benchmark: no .csproj above {}.", path.display()));
            self.show_panel(PanelView::Output, cx);
            return;
        };
        self.save_all(window, cx);
        self.log(format!(
            "Running {} in Release. Its results show in the Benchmarks tab when it finishes (BenchmarkDotNet takes the filter when Main passes its arguments on: BenchmarkSwitcher.FromAssembly(typeof(Program).Assembly).Run(args)).",
            run.full_name
        ));
        let dir = project.parent().map(Path::to_path_buf);
        let title = format!("Benchmark: {}", run.method.clone().unwrap_or_else(|| run.full_name.rsplit('.').next().unwrap_or_default().to_owned()));
        self.run_in_terminal(title, bench::command(&project, &run), dir, window, cx);
        self.benchmarks.expecting = true;
    }

    /// "Run Benchmark at Cursor".
    pub(super) fn run_benchmark_at_cursor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, line)) = self.active.and_then(|i| self.tabs.get(i)).and_then(|t| Some((t.path.clone()?, t.editor.read(cx).state.caret().line))) else {
            return;
        };
        self.run_benchmark_at(&path, line, window, cx);
    }

    /// Reads the workspace's benchmark reports again (off the UI thread). New ones go into the history;
    /// when a run started here has new results, the Benchmarks tab comes up.
    pub(super) fn reload_benchmarks(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.root() else { return };
        if self.benchmarks.history.is_none() {
            let saved = self.state.get(HISTORY_KEY).and_then(|t| serde_json::from_str::<History>(&t).ok());
            self.benchmarks.history = Some(saved.unwrap_or_default());
        }
        self.benchmarks.loading = Some(cx.spawn(async move |this, cx| {
            let reports = cx
                .background_executor()
                .spawn(async move {
                    let mut reports: Vec<(PathBuf, SystemTime, Vec<BenchmarkResult>)> = bench::find_reports(&root)
                        .into_iter()
                        .map(|(file, written)| {
                            let results = std::fs::read_to_string(&file).map(|t| bench::parse_report(&t)).unwrap_or_default();
                            (file, written, results)
                        })
                        .filter(|(_, _, r)| !r.is_empty())
                        .collect();
                    reports.sort_by_key(|r| std::cmp::Reverse(r.1));
                    reports
                })
                .await;
            let _ = this.update(cx, |wb, cx| {
                wb.benchmarks.loading = None;
                let history = wb.benchmarks.history.get_or_insert_with(History::default);
                // Oldest first, so each run's "previous" is the run before it.
                let mut changed = false;
                for (_, written, results) in reports.iter().rev() {
                    changed |= history.record(results, *written);
                }
                if changed {
                    let saved = serde_json::to_string(history).ok();
                    wb.state.set(HISTORY_KEY, saved.as_deref());
                    if wb.benchmarks.expecting && !reports.is_empty() {
                        wb.benchmarks.expecting = false;
                        wb.benchmarks.reports = reports;
                        wb.show_panel(PanelView::Benchmarks, cx);
                        return;
                    }
                }
                wb.benchmarks.reports = reports;
                cx.notify();
            });
        }));
    }

    /// The Benchmarks panel tab: each report (a benchmark class's last run), newest first.
    pub(super) fn render_benchmarks(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = Theme::global(cx);
        let muted = t.c("foreground.muted");
        let (faster, slower) = (t.c_or("testing.iconPassed", 0x73c991ff), t.c_or("testing.iconFailed", 0xf14c4cff));
        let mono = t.editor_font.clone();
        let cell = |w: f32, text: String| div().w(px(w)).flex_none().whitespace_nowrap().overflow_hidden().flex().justify_end().child(text);
        let header = div()
            .flex()
            .gap_3()
            .px_3()
            .py_1()
            .text_size(px(11.))
            .text_color(muted)
            .child(div().flex_1().min_w_0().child("BENCHMARK"))
            .child(cell(100., "MEAN".into()))
            .child(cell(90., "ERROR".into()))
            .child(cell(90., "ALLOCATED".into()))
            .child(cell(90., "CHANGE".into()));
        let mut list = div().id("benchmark-results").flex_1().min_h_0().overflow_y_scroll().flex().flex_col();
        if self.benchmarks.reports.is_empty() {
            list = list.child(div().p_3().text_color(muted).child("No results yet: run a benchmark (the \"Run Benchmark\" lens above a [Benchmark] method, or right-click → Run Benchmark at Cursor)."));
        }
        let history = self.benchmarks.history.clone().unwrap_or_default();
        for (ri, (file, written, results)) in self.benchmarks.reports.iter().enumerate() {
            let name = file.file_name().map(|n| n.to_string_lossy().trim_end_matches("-report-full.json").to_owned()).unwrap_or_default();
            let age = written.elapsed().map(|d| age_text(d.as_secs())).unwrap_or_default();
            list = list.child(
                div()
                    .px_3()
                    .pt(px(if ri == 0 { 2. } else { 8. }))
                    .flex()
                    .gap_2()
                    .text_size(px(11.))
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(name))
                    .child(div().text_color(muted).child(format!("{age} ago"))),
            );
            for (bi, r) in results.iter().enumerate() {
                let change = history.change(r);
                let (change_text, change_color) = match change {
                    Some(c) if c.abs() < 0.02 => (format!("{:+.1}%", c * 100.), muted),
                    Some(c) if c < 0.0 => (format!("{:+.1}%", c * 100.), faster),
                    Some(c) => (format!("{:+.1}%", c * 100.), slower),
                    None => ("—".to_owned(), muted),
                };
                list = list.child(
                    div()
                        .id(SharedString::from(format!("bench-{ri}-{bi}")))
                        .debug_selector(move || format!("bench-{ri}-{bi}"))
                        .flex()
                        .gap_3()
                        .px_3()
                        .py(px(2.))
                        .hover(|s| s.bg(t.c("list.hoverBackground")))
                        .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(r.display.clone()))
                        .child(cell(100., r.mean.map_or_else(|| "failed".to_owned(), bench::format_time)).font_family(mono.clone()))
                        .child(cell(90., r.error.map(bench::format_time).unwrap_or_default()).font_family(mono.clone()).text_color(muted))
                        .child(cell(90., r.allocated.map(bench::format_bytes).unwrap_or_else(|| "—".into())).font_family(mono.clone()))
                        .child(cell(90., change_text).font_family(mono.clone()).text_color(change_color)),
                );
            }
        }
        div().size_full().flex().flex_col().text_size(t.m("list.fontSize", 13.0)).child(header).child(list).into_any_element()
    }
}

/// "12s", "5m", "3h", "2d".
fn age_text(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        3600..86400 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86400),
    }
}

#[cfg(test)]
mod view_tests {
    use crate::workbench::settings_page::view_tests::workbench;

    /// Reports in the workspace show in the tab; a second, newer run shows the change since the first.
    #[gpui::test]
    fn the_benchmarks_tab_shows_results_and_the_change(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "benchmarks-tab");
        let root = guard.dir.join("bench");
        let results = root.join("Bench/BenchmarkDotNet.Artifacts/results");
        std::fs::create_dir_all(&results).unwrap();
        let report = |mean: f64| {
            format!(r#"{{"Benchmarks":[{{"FullName":"Bench.Sorts.ArraySort","Type":"Sorts","Method":"ArraySort","Parameters":"","Statistics":{{"Mean":{mean},"ConfidenceInterval":{{"Margin":1.5}}}},"Memory":{{"BytesAllocatedPerOperation":64}}}}]}}"#)
        };
        let file = results.join("Bench.Sorts-report-full.json");
        std::fs::write(&file, report(1000.0)).unwrap();
        wb.update_in(cx, |wb, window, cx| wb.open_folder(root.clone(), window, cx));
        let load = |cx: &mut gpui::VisualTestContext| {
            wb.update(cx, |wb, cx| wb.reload_benchmarks(cx));
            for _ in 0..200 {
                cx.run_until_parked();
                if wb.update(cx, |wb, _| wb.benchmarks.loading.is_none()) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        load(cx);
        assert!(wb.update(cx, |wb, _| wb.benchmarks.has_results()));
        // A newer run, 20% faster (the file's time decides what's newer: one second later).
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(&file, report(800.0)).unwrap();
        wb.update(cx, |wb, _| wb.benchmarks.expecting = true);
        load(cx);
        let (panel, change) = wb.update(cx, |wb, _| {
            let r = &wb.benchmarks.reports[0].2[0];
            (wb.panel, wb.benchmarks.history.as_ref().unwrap().change(r))
        });
        assert!(panel == super::PanelView::Benchmarks, "the tab comes up for the run started here");
        assert!(change.is_some_and(|c| (c + 0.2).abs() < 1e-9), "{change:?}");
        assert!(cx.debug_bounds("bench-0-0").is_some(), "the result row is drawn");
        drop(guard);
    }
}
