//! Live metrics: the running .NET processes and, for the one picked (the app being run or debugged on
//! its own), CPU, memory, GC, thread pool, exceptions and requests as sparklines in the Metrics panel tab,
//! read from `dotnet-counters` (see `bf_core::metrics`). While the app runs, the status bar shows its CPU
//! and memory.

use std::collections::VecDeque;

use bf_core::debug::RuntimeProcess;
use bf_core::metrics::{self, CountersSession, Metric, MetricsEvent, Sample};

use super::*;

/// How many seconds the sparklines show.
const HISTORY: usize = 120;

/// The setting that turns watching the app being run off.
const SETTING: &str = "metrics.statusBar";

#[derive(Default)]
pub(super) struct MetricsState {
    /// Where `dotnet-counters` is (looked up again until found).
    tool: Option<PathBuf>,
    processes: Vec<RuntimeProcess>,
    listing: Option<Task<()>>,
    /// The project name of the app being run or debugged, to find its process.
    app: Option<String>,
    /// The process of that app, once found.
    app_pid: Option<u32>,
    watched: Option<Watched>,
    /// The last samples, oldest first.
    history: VecDeque<Sample>,
    /// (pid, name) the history is of.
    subject: Option<(u32, String)>,
    /// Why monitoring ended (or couldn't start).
    note: Option<String>,
    /// Looking for the app's process after a run started.
    finding: Option<Task<()>>,
}

struct Watched {
    pid: u32,
    session: CountersSession,
    _task: Task<()>,
}

impl MetricsState {
    /// CPU and memory of the process being watched, for the status bar.
    pub(super) fn status_text(&self) -> Option<String> {
        self.watched.as_ref()?;
        let last = self.history.back()?;
        let cpu = last.get(Metric::Cpu).map(|v| format!("{v:.0}%"));
        let mem = last.get(Metric::WorkingSet).map(metrics::format_bytes);
        let parts: Vec<String> = [cpu, mem].into_iter().flatten().collect();
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    pub(super) fn has_subject(&self) -> bool {
        self.subject.is_some()
    }

    fn tool(&mut self) -> Option<PathBuf> {
        if self.tool.as_ref().is_none_or(|t| !t.is_file()) {
            self.tool = metrics::counters_tool();
        }
        self.tool.clone()
    }
}

impl Workbench {
    /// "Show Metrics": the panel tab, with the process list read again.
    pub(super) fn show_metrics(&mut self, cx: &mut Context<Self>) {
        self.show_panel(PanelView::Metrics, cx);
        self.list_metrics_processes(cx);
    }

    /// Reads the running .NET processes again (off the UI thread).
    pub(super) fn list_metrics_processes(&mut self, cx: &mut Context<Self>) {
        self.metrics.listing = Some(cx.spawn(async move |this, cx| {
            let processes = cx.background_executor().spawn(async { bf_core::debug::dotnet_processes() }).await;
            let _ = this.update(cx, |wb, cx| {
                wb.metrics.listing = None;
                if let Some(app) = wb.metrics.app.clone()
                    && wb.app_running()
                {
                    wb.metrics.app_pid = metrics::app_process(&processes, &app).or(wb.metrics.app_pid);
                }
                wb.metrics.processes = processes;
                cx.notify();
            });
        }));
    }

    /// The running .NET processes, as last listed (the Profiler shows them too).
    pub(super) fn metrics_processes(&self) -> &[RuntimeProcess] {
        &self.metrics.processes
    }

    /// The process of the app being run or debugged, while it runs.
    pub(super) fn metrics_app_pid(&self) -> Option<u32> {
        self.metrics.app_pid.filter(|_| self.app_running())
    }

    fn app_running(&self) -> bool {
        self.dotnet.as_ref().is_some_and(DotnetProcess::is_running) || self.debug.is_some()
    }

    /// A run or debug session of `project` started: once its app's process is up, its metrics are
    /// watched (when dotnet-counters is installed and the setting allows it), for the status bar.
    pub(super) fn watch_app_metrics(&mut self, project: &Path, cx: &mut Context<Self>) {
        let Some(app) = project.file_stem().map(|s| s.to_string_lossy().into_owned()) else { return };
        self.metrics.app = Some(app.clone());
        self.metrics.app_pid = None;
        if !self.settings.get(SETTING, true) || self.metrics.tool().is_none() {
            return;
        }
        self.metrics.finding = Some(cx.spawn(async move |this, cx| {
            // Building comes first; give up after a few minutes.
            for _ in 0..180 {
                cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
                let Ok(running) = this.update(cx, |wb, _| wb.app_running()) else { return };
                if !running {
                    break;
                }
                let wanted = app.clone();
                let found = cx.background_executor().spawn(async move { metrics::app_process(&bf_core::debug::dotnet_processes(), &wanted) }).await;
                if let Some(pid) = found {
                    let _ = this.update(cx, |wb, cx| {
                        wb.metrics.app_pid = Some(pid);
                        wb.metrics.finding = None;
                        // Watching something else on purpose: leave it.
                        if wb.metrics.watched.is_none() {
                            wb.monitor_process(pid, app.clone(), cx);
                        }
                    });
                    return;
                }
            }
            let _ = this.update(cx, |wb, _| wb.metrics.finding = None);
        }));
    }

    /// Starts watching `pid` (stops watching the one before).
    pub(super) fn monitor_process(&mut self, pid: u32, name: String, cx: &mut Context<Self>) {
        self.stop_monitoring(cx);
        let Some(tool) = self.metrics.tool() else {
            self.metrics.note = Some("dotnet-counters isn't installed.".into());
            cx.notify();
            return;
        };
        if self.metrics.subject.as_ref().is_none_or(|(p, _)| *p != pid) {
            self.metrics.history.clear();
        }
        self.metrics.subject = Some((pid, name.clone()));
        self.metrics.note = None;
        match CountersSession::start(&tool, pid) {
            Ok((session, rx)) => {
                let task = cx.spawn(async move |this, cx| {
                    let mut samples = 0usize;
                    while let Ok(event) = rx.recv_async().await {
                        let alive = this
                            .update(cx, |wb, cx| {
                                match event {
                                    MetricsEvent::Sample(sample) => {
                                        samples += 1;
                                        let history = &mut wb.metrics.history;
                                        history.push_back(sample);
                                        while history.len() > HISTORY {
                                            history.pop_front();
                                        }
                                    }
                                    MetricsEvent::Ended(said) => {
                                        if wb.metrics.watched.as_ref().is_some_and(|w| w.pid == pid) {
                                            wb.metrics.watched = None;
                                            wb.metrics.note = Some(if samples > 0 {
                                                format!("{name} ({pid}) exited.")
                                            } else if said.is_empty() {
                                                format!("dotnet-counters couldn't read {name} ({pid}).")
                                            } else {
                                                format!("dotnet-counters couldn't read {name} ({pid}): {said}")
                                            });
                                        }
                                    }
                                }
                                cx.notify();
                            })
                            .is_ok();
                        if !alive {
                            break;
                        }
                    }
                });
                self.metrics.watched = Some(Watched { pid, session, _task: task });
            }
            Err(e) => self.metrics.note = Some(format!("Could not start dotnet-counters: {e}")),
        }
        cx.notify();
    }

    pub(super) fn stop_monitoring(&mut self, cx: &mut Context<Self>) {
        if let Some(w) = self.metrics.watched.take() {
            w.session.stop();
        }
        cx.notify();
    }

    /// Offers to install dotnet-counters as a global .NET tool.
    fn install_counters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.workspace.root().unwrap_or_else(std::env::temp_dir);
        let args = vec!["tool".into(), "install".into(), "--global".into(), "dotnet-counters".into()];
        let then: DotnetDone = Box::new(|wb, ok, _, cx| {
            if ok {
                wb.log("dotnet-counters is installed. Pick a process in the Metrics tab.");
                wb.show_metrics(cx);
            } else {
                wb.log("Installing dotnet-counters failed; see the output above.");
                wb.show_panel(PanelView::Output, cx);
            }
        });
        self.show_panel(PanelView::Output, cx);
        self.start_dotnet(root, args, Some(then), window, cx);
    }

    /// The Metrics panel tab: the processes on the left, the sparklines of the one watched on the right.
    pub(super) fn render_metrics(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = Theme::global(cx);
        let muted = t.c("foreground.muted");
        let has_tool = self.metrics.tool().is_some();
        let small = |id: &'static str, name: &str, tip: &str| {
            div()
                .id(id)
                .debug_selector(move || id.to_owned())
                .p(px(2.))
                .rounded(px(3.))
                .cursor_pointer()
                .hover(|s| s.bg(t.c("toolbar.hoverBackground")))
                .tooltip(tooltip(tip.to_owned()))
                .child(icon(name, px(14.), t.c("icon.foreground")))
        };

        // The processes, the app being run first.
        let watched = self.metrics.watched.as_ref().map(|w| w.pid);
        let app_pid = self.metrics.app_pid.filter(|_| self.app_running());
        let mut processes = self.metrics.processes.clone();
        processes.sort_by_key(|p| (Some(p.pid) != app_pid, p.pid));
        let mut list = div().id("metrics-processes").flex_1().min_h_0().overflow_y_scroll().flex().flex_col();
        if processes.is_empty() {
            list = list.child(div().px_3().py_1().text_size(px(12.)).text_color(muted).child(if self.metrics.listing.is_some() { "Looking for .NET processes…" } else { "No .NET process is running." }));
        }
        for p in processes {
            let (pid, name) = (p.pid, p.name.clone());
            let selected = watched == Some(pid);
            let label = if Some(pid) == app_pid { format!("{name}  (running here)") } else { name.clone() };
            list = list.child(
                div()
                    .id(SharedString::from(format!("metrics-proc-{pid}")))
                    .debug_selector(move || format!("metrics-proc-{pid}"))
                    .px_3()
                    .py(px(2.))
                    .flex()
                    .gap_2()
                    .cursor_pointer()
                    .when(selected, |d| d.bg(t.c("list.activeSelectionBackground")))
                    .hover(|s| s.bg(t.c("list.hoverBackground")))
                    .tooltip(tooltip(p.command_line.clone()))
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(label))
                    .child(div().flex_none().text_color(muted).child(pid.to_string()))
                    .on_click(cx.listener(move |wb, _, _, cx| wb.monitor_process(pid, name.clone(), cx))),
            );
        }
        let left = div()
            .w(px(260.))
            .flex_none()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(t.c("panel.border"))
            .child(
                div()
                    .px_3()
                    .py_1()
                    .flex()
                    .items_center()
                    .text_size(px(11.))
                    .text_color(muted)
                    .child(div().flex_1().child(".NET PROCESSES"))
                    .child(small("metrics-refresh", "refresh", "Refresh").on_click(cx.listener(|wb, _, _, cx| wb.list_metrics_processes(cx)))),
            )
            .child(list);

        // The sparklines.
        let mut right = div().flex_1().min_w_0().flex().flex_col().overflow_hidden();
        if !has_tool {
            right = right.child(
                div()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .items_start()
                    .child(div().text_color(muted).child("Live metrics come from dotnet-counters, a .NET global tool that isn't installed."))
                    .child(
                        div()
                            .id("metrics-install")
                            .debug_selector(|| "metrics-install".into())
                            .px_2()
                            .py(px(3.))
                            .rounded(px(3.))
                            .cursor_pointer()
                            .bg(t.c("button.background"))
                            .text_color(t.c("button.foreground"))
                            .hover(|s| s.bg(t.c("button.hoverBackground")))
                            .child("Install dotnet-counters")
                            .on_click(cx.listener(|wb, _, w, cx| wb.install_counters(w, cx))),
                    ),
            );
        } else if let Some((pid, name)) = self.metrics.subject.clone() {
            let mut title = div()
                .px_3()
                .py_1()
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(11.))
                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(format!("{name} ({pid})")))
                .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_color(muted).child(match (&self.metrics.note, watched) {
                    (Some(note), _) => note.clone(),
                    (None, Some(_)) if self.metrics.history.is_empty() => "Connecting…".to_owned(),
                    (None, Some(_)) => "Live, once a second".to_owned(),
                    (None, None) => "Stopped".to_owned(),
                }));
            if watched.is_some() {
                title = title.child(small("metrics-stop", "stop", "Stop watching").on_click(cx.listener(|wb, _, _, cx| wb.stop_monitoring(cx))));
            }
            let mut cards = div().id("metrics-cards").flex_1().min_h_0().overflow_y_scroll().px_3().pb_2().flex().flex_wrap().gap_2();
            for metric in Metric::ALL {
                let values: Vec<Option<f64>> = self.metrics.history.iter().map(|s| s.get(metric)).collect();
                let current = values.iter().rev().flatten().next().copied();
                // Requests only for web apps; the rest always.
                if metric == Metric::Requests && current.is_none() {
                    continue;
                }
                let color = t.c_or("charts.blue", 0x3794ffff);
                let id = format!("metric-{metric:?}");
                cards = cards.child(
                    div()
                        .id(SharedString::from(id.clone()))
                        .debug_selector(move || id.clone())
                        .w(px(210.))
                        .p_2()
                        .rounded(px(4.))
                        .border_1()
                        .border_color(t.c("panel.border"))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .flex()
                                .text_size(px(11.))
                                .child(div().flex_1().text_color(muted).child(metric.label()))
                                .child(div().font_family(t.editor_font.clone()).child(current.map_or_else(|| "—".to_owned(), |v| metric.format(v)))),
                        )
                        .child(sparkline(values, color)),
                );
            }
            right = right.child(title).child(cards);
        } else {
            right = right.child(div().p_3().text_color(muted).child(match &self.metrics.note {
                Some(note) => note.clone(),
                None => "Pick a process on the left to see its CPU, memory, GC, thread pool, exceptions and requests. The app you run or debug is watched by itself.".to_owned(),
            }));
        }
        div().size_full().flex().text_size(t.m("list.fontSize", 13.0)).child(left).child(right).into_any_element()
    }
}

/// A line of `values` (gaps where a second had none), scaled to the largest.
fn sparkline(values: Vec<Option<f64>>, color: gpui::Hsla) -> impl IntoElement {
    gpui::canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let max = values.iter().flatten().fold(0.0f64, |m, v| m.max(*v));
            let max = if max <= 0.0 { 1.0 } else { max * 1.1 };
            let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            let step = w / (HISTORY - 1) as f32;
            // Right-aligned: the newest second at the right edge.
            let offset = HISTORY.saturating_sub(values.len());
            let mut builder = gpui::PathBuilder::stroke(px(1.5));
            let mut drawing = false;
            let mut any = false;
            for (i, v) in values.iter().enumerate() {
                let Some(v) = v else {
                    drawing = false;
                    continue;
                };
                let x = bounds.origin.x + px((offset + i) as f32 * step);
                let y = bounds.origin.y + px(h - 1.0 - (*v / max) as f32 * (h - 2.0));
                if drawing {
                    builder.line_to(gpui::point(x, y));
                    any = true;
                } else {
                    builder.move_to(gpui::point(x, y));
                    drawing = true;
                }
            }
            if any && let Ok(path) = builder.build() {
                window.paint_path(path, color);
            }
        },
    )
    .w_full()
    .h(px(36.))
}

#[cfg(test)]
mod view_tests {
    use crate::workbench::settings_page::view_tests::workbench;

    /// The tab lists processes and draws a card per metric for the samples that came in.
    #[gpui::test]
    fn the_metrics_tab_shows_sparklines(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "metrics-tab");
        wb.update(cx, |wb, cx| wb.show_panel(super::PanelView::Metrics, cx));
        cx.run_until_parked();
        wb.update(cx, |wb, cx| {
            wb.metrics.listing = None;
            wb.metrics.tool = Some(std::env::current_exe().unwrap());
            wb.metrics.processes = vec![bf_core::debug::RuntimeProcess { pid: 4242, name: "WebShop".into(), command_line: "/w/WebShop".into() }];
            wb.metrics.subject = Some((4242, "WebShop".into()));
            let mut a = bf_core::metrics::Aggregator::new(1.0);
            for (ts, cpu) in [("1", 10.0), ("2", 30.0), ("3", 20.0)] {
                let e = bf_core::metrics::CounterEvent { timestamp: ts.into(), name: "CPU Usage (%)".into(), tags: String::new(), value: cpu };
                if let Some(s) = a.push(&e) {
                    wb.metrics.history.push_back(s);
                }
                let e = bf_core::metrics::CounterEvent { timestamp: ts.into(), name: "Working Set (MB)".into(), tags: String::new(), value: 100.0 };
                a.push(&e);
            }
            wb.metrics.history.push_back(a.flush().unwrap());
            cx.notify();
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("metrics-proc-4242").is_some(), "the process is listed");
        assert!(cx.debug_bounds("metric-Cpu").is_some(), "a CPU card");
        assert!(cx.debug_bounds("metric-Requests").is_none(), "no requests card for an app without them");
        // Not watched: no status bar text.
        assert_eq!(wb.update(cx, |wb, _| wb.metrics.status_text()), None);
        drop(guard);
    }
}
