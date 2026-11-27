//! The Profiler panel tab: CPU profiles of a running .NET process recorded with `dotnet-trace` (hot
//! methods, call tree, flame graph) and heap snapshots taken with `dotnet-gcdump` (types by size, with
//! the change since the snapshot before). See `bf_core::profiling`.
//!
//! dotnet-trace stops (and resolves the method names) when Enter is pressed in its console, so it runs on
//! a terminal of its own, out of sight; Stop presses Enter there.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use bf_core::profiling::{self, HeapReport, Profile};

use super::*;

/// What the Profiler shows.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(super) enum ProfilerView {
    #[default]
    HotMethods,
    CallTree,
    FlameGraph,
    Memory,
}

/// Rows a table shows at most.
const MAX_ROWS: usize = 500;
/// Height of a flame graph row.
const FLAME_ROW: f32 = 20.0;

#[derive(Default)]
pub(super) struct ProfilerState {
    /// The process picked on the left (else the app being run).
    target: Option<(u32, String)>,
    recording: Option<Recording>,
    profile: Option<Arc<Profile>>,
    /// Each node's parent (for the flame graph's zoom).
    parents: Vec<usize>,
    /// "Busy (1234), 5.0 s".
    profile_of: String,
    pub(super) view: ProfilerView,
    /// Show all time, also waiting (else only time running .NET code).
    all_time: bool,
    /// Only methods and types of the workspace's own projects.
    my_code: bool,
    /// The workspace's assembly names, lower case.
    user_modules: HashSet<String>,
    /// Call tree nodes that are open.
    expanded: HashSet<usize>,
    /// The flame graph's top node (zoomed in).
    zoom: Option<usize>,
    heap: Option<HeapReport>,
    heap_before: Option<HeapReport>,
    heap_of: String,
    /// Loading a trace or taking a snapshot.
    busy: Option<(String, Task<()>)>,
    /// Why something didn't work, or what to do next.
    note: Option<String>,
    /// Ticks once a second while recording (the elapsed time).
    _ticker: Option<Task<()>>,
    runs: u64,
}

struct Recording {
    pid: u32,
    name: String,
    terminal: Entity<TerminalView>,
    _sub: Subscription,
    out: PathBuf,
    started: Instant,
    stopping: bool,
}

impl ProfilerState {
    pub(super) fn has_content(&self) -> bool {
        self.profile.is_some() || self.heap.is_some() || self.recording.is_some()
    }

    pub(super) fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    fn is_user(&self, module: &str) -> bool {
        self.user_modules.contains(&module.to_ascii_lowercase())
    }
}

impl Workbench {
    pub(super) fn show_profiler(&mut self, cx: &mut Context<Self>) {
        self.show_panel(PanelView::Profiler, cx);
        self.list_metrics_processes(cx);
    }

    /// The process to profile: the one picked, else the app being run or debugged.
    fn profiler_target(&self) -> Option<(u32, String)> {
        self.profiler.target.clone().or_else(|| {
            let pid = self.metrics_app_pid()?;
            let name = self.metrics_processes().iter().find(|p| p.pid == pid).map(|p| p.name.clone()).unwrap_or_else(|| "app".into());
            Some((pid, name))
        })
    }

    fn pick_profiler_target(&mut self, pid: u32, name: String, cx: &mut Context<Self>) {
        self.profiler.target = Some((pid, name));
        self.profiler.note = None;
        cx.notify();
    }

    /// "Record CPU Profile": starts dotnet-trace on the target; again: stops it.
    pub(super) fn toggle_cpu_profile(&mut self, cx: &mut Context<Self>) {
        if self.profiler.recording.is_some() {
            self.stop_cpu_profile(cx);
        } else {
            self.start_cpu_profile(cx);
        }
    }

    fn start_cpu_profile(&mut self, cx: &mut Context<Self>) {
        self.show_profiler(cx);
        let Some(tool) = profiling::global_tool(profiling::TRACE_TOOL) else {
            self.profiler.note = Some("Recording needs dotnet-trace, a .NET global tool that isn't installed.".into());
            return cx.notify();
        };
        let Some((pid, name)) = self.profiler_target() else {
            self.profiler.note = Some("Pick the process to profile on the left (or start the app first).".into());
            return cx.notify();
        };
        self.profiler.runs += 1;
        let out = std::env::temp_dir().join(format!("byteforge-trace-{}-{pid}-{}.nettrace", std::process::id(), self.profiler.runs));
        let _ = std::fs::remove_file(&out);
        let _ = std::fs::remove_file(profiling::speedscope_path(&out));
        let spawned = match crate::terminal::spawn(Some(tool.to_string_lossy().into_owned()), profiling::trace_args(pid, &out), self.workspace.root()) {
            Ok(s) => s,
            Err(e) => {
                self.profiler.note = Some(format!("Could not start dotnet-trace: {e:#}"));
                return cx.notify();
            }
        };
        let terminal = cx.new(|cx| TerminalView::new(spawned, cx));
        let sub = cx.subscribe(&terminal, |wb: &mut Self, _, event: &TerminalEvent, cx| {
            if matches!(event, TerminalEvent::Exited) {
                wb.finish_cpu_profile(cx);
            }
        });
        self.profiler.note = None;
        self.profiler.recording = Some(Recording { pid, name, terminal, _sub: sub, out, started: Instant::now(), stopping: false });
        self.profiler._ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        }));
        cx.notify();
    }

    /// Presses Enter in dotnet-trace's console: it stops, resolves the method names and writes the file.
    pub(super) fn stop_cpu_profile(&mut self, cx: &mut Context<Self>) {
        if let Some(rec) = self.profiler.recording.as_mut()
            && !rec.stopping
        {
            rec.stopping = true;
            rec.terminal.update(cx, |t, cx| t.run_command("", cx));
            cx.notify();
        }
    }

    /// dotnet-trace ended (stopped, or the process exited): reads its Speedscope file.
    fn finish_cpu_profile(&mut self, cx: &mut Context<Self>) {
        let Some(rec) = self.profiler.recording.take() else { return };
        self.profiler._ticker = None;
        let file = profiling::speedscope_path(&rec.out);
        if !file.is_file() {
            let said = rec.terminal.read(cx).screen_text();
            let said = said.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("").trim().to_owned();
            self.profiler.note = Some(if said.is_empty() {
                format!("dotnet-trace ended without a trace of {} ({}).", rec.name, rec.pid)
            } else {
                format!("dotnet-trace ended without a trace of {} ({}): {said}", rec.name, rec.pid)
            });
            return cx.notify();
        }
        let label = format!("{} ({})", rec.name, rec.pid);
        let root = self.workspace.root();
        let task = cx.spawn(async move |this, cx| {
            let parsed = cx
                .background_executor()
                .spawn(async move {
                    let profile = std::fs::read_to_string(&file).map_err(anyhow::Error::from).and_then(|json| profiling::parse_speedscope(&json));
                    let modules: HashSet<String> = root
                        .map(|r| bf_core::dotnet::projects(&r).iter().filter_map(|p| p.file_stem()).map(|s| s.to_string_lossy().to_ascii_lowercase()).collect())
                        .unwrap_or_default();
                    (profile, modules, file)
                })
                .await;
            let _ = this.update(cx, |wb, cx| {
                wb.profiler.busy = None;
                let (profile, modules, file) = parsed;
                match profile {
                    Ok(profile) => {
                        let secs = profile.duration_ms / 1000.0;
                        wb.profiler.profile_of = format!("{label} · {secs:.1} s · {} thread{}", profile.threads, if profile.threads == 1 { "" } else { "s" });
                        wb.log(format!("CPU profile of {label} ({secs:.1} s); the trace is in {}", file.with_extension("").with_extension("nettrace").display()));
                        wb.set_profile(profile, modules);
                    }
                    Err(e) => wb.profiler.note = Some(format!("Could not read the trace: {e:#}")),
                }
                cx.notify();
            });
        });
        self.profiler.busy = Some(("Reading the trace…".into(), task));
        cx.notify();
    }

    fn set_profile(&mut self, profile: Profile, modules: HashSet<String>) {
        let mut parents = vec![0; profile.nodes.len()];
        for (i, node) in profile.nodes.iter().enumerate() {
            for &c in &node.children {
                parents[c] = i;
            }
        }
        let p = &mut self.profiler;
        p.expanded = profiling::hot_path(&profile, !p.all_time, 0.05).into_iter().collect();
        p.expanded.insert(0);
        p.zoom = None;
        p.parents = parents;
        p.user_modules = modules;
        p.profile = Some(Arc::new(profile));
        p.note = None;
        if p.view == ProfilerView::Memory {
            p.view = ProfilerView::HotMethods;
        }
    }

    /// "Take Memory Snapshot": dotnet-gcdump's report of the target's heap; the one before stays to compare.
    pub(super) fn take_memory_snapshot(&mut self, cx: &mut Context<Self>) {
        self.show_profiler(cx);
        let Some(tool) = profiling::global_tool(profiling::GCDUMP_TOOL) else {
            self.profiler.note = Some("Memory snapshots need dotnet-gcdump, a .NET global tool that isn't installed.".into());
            return cx.notify();
        };
        let Some((pid, name)) = self.profiler_target() else {
            self.profiler.note = Some("Pick the process on the left (or start the app first).".into());
            return cx.notify();
        };
        self.profiler.runs += 1;
        let file = std::env::temp_dir().join(format!("byteforge-heap-{}-{pid}-{}.gcdump", std::process::id(), self.profiler.runs));
        let root = self.workspace.root();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let run = |args: Vec<String>| -> anyhow::Result<String> {
                        let mut command = bf_core::process::command(&tool);
                        command.args(args).stdin(std::process::Stdio::null());
                        for (k, v) in profiling::tool_env() {
                            command.env(k, v);
                        }
                        let out = command.output()?;
                        let text = String::from_utf8_lossy(&out.stdout).into_owned();
                        if !out.status.success() {
                            let err = String::from_utf8_lossy(&out.stderr);
                            let said = format!("{text}\n{err}");
                            anyhow::bail!("{}", said.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("dotnet-gcdump failed"));
                        }
                        Ok(text)
                    };
                    run(profiling::gcdump_collect_args(pid, &file))?;
                    let report = profiling::parse_gcdump_report(&run(profiling::gcdump_report_args(&file))?);
                    let _ = std::fs::remove_file(&file);
                    let modules: HashSet<String> = root
                        .map(|r| bf_core::dotnet::projects(&r).iter().filter_map(|p| p.file_stem()).map(|s| s.to_string_lossy().to_ascii_lowercase()).collect())
                        .unwrap_or_default();
                    anyhow::Ok((report, modules))
                })
                .await;
            let _ = this.update(cx, |wb, cx| {
                wb.profiler.busy = None;
                match result {
                    Ok((report, modules)) => wb.set_heap(report, modules, format!("{name} ({pid})")),
                    Err(e) => wb.profiler.note = Some(format!("Could not take a memory snapshot of {name} ({pid}): {e:#}")),
                }
                cx.notify();
            });
        });
        self.profiler.busy = Some(("Taking a memory snapshot…".into(), task));
        cx.notify();
    }

    fn set_heap(&mut self, report: HeapReport, modules: HashSet<String>, of: String) {
        let p = &mut self.profiler;
        // Comparing makes sense with a snapshot of the same process only.
        p.heap_before = if p.heap_of == of { p.heap.take() } else { None };
        p.heap = Some(report);
        p.heap_of = of;
        p.user_modules.extend(modules);
        p.view = ProfilerView::Memory;
        p.note = None;
    }

    /// Installs dotnet-trace and dotnet-gcdump (those missing) as global .NET tools.
    fn install_profiling_tools(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let missing: Vec<&str> = [profiling::TRACE_TOOL, profiling::GCDUMP_TOOL].into_iter().filter(|t| profiling::global_tool(t).is_none()).collect();
        let Some((&first, rest)) = missing.split_first() else { return };
        let rest: Vec<String> = rest.iter().map(|s| s.to_string()).collect();
        let root = self.workspace.root().unwrap_or_else(std::env::temp_dir);
        let args = vec!["tool".into(), "install".into(), "--global".into(), first.to_owned()];
        let next_root = root.clone();
        let then: DotnetDone = Box::new(move |wb, ok, window, cx| {
            if !ok {
                wb.log(format!("Installing {first} failed; see the output above."));
                return;
            }
            if let Some(next) = rest.first() {
                let args = vec!["tool".into(), "install".into(), "--global".into(), next.clone()];
                let done: DotnetDone = Box::new(|wb, ok, _, cx| {
                    wb.log(if ok { "The profiling tools are installed." } else { "Installing a profiling tool failed; see the output above." });
                    wb.show_profiler(cx);
                });
                wb.start_dotnet(next_root, args, Some(done), window, cx);
            } else {
                wb.log("The profiling tools are installed.");
                wb.show_profiler(cx);
            }
        });
        self.show_panel(PanelView::Output, cx);
        self.start_dotnet(root, args, Some(then), window, cx);
    }

    /// Opens a profiled method's source: the language server's workspace symbol with its name in its type.
    fn open_profiled_method(&mut self, frame: usize, cx: &mut Context<Self>) {
        let Some(profile) = self.profiler.profile.clone() else { return };
        let Some(f) = profile.frames.get(frame) else { return };
        let Some((owner, method)) = f.source_name() else {
            self.log(format!("{} has no source to open (generated or runtime code).", f.short()));
            return;
        };
        let Some(lsp) = self.lsp.clone() else {
            self.log("Opening a method's source needs the C# language server.");
            return;
        };
        let rx = lsp.workspace_symbols(&method);
        let short = f.short();
        self.symbol_task = Some(cx.spawn(async move |this, cx| {
            let symbols = rx.recv_async().await.ok().and_then(Result::ok).unwrap_or_default();
            let _ = this.update(cx, |wb, cx| {
                let name_matches = |s: &bf_lsp::Symbol| s.name == method || s.name.starts_with(&format!("{method}("));
                let owner_matches = |s: &bf_lsp::Symbol| s.container.as_deref().is_some_and(|c| c == owner || c.ends_with(&format!(".{owner}")));
                let found = symbols.iter().find(|s| name_matches(s) && owner_matches(s)).or_else(|| symbols.iter().find(|s| name_matches(s)));
                match found {
                    Some(s) => {
                        let (path, line, ch) = (s.path.clone(), s.range.start.line, s.range.start.character);
                        Self::with_window(cx, move |wb, window, cx| {
                            let at = wb.symbol_point(&path, line, ch, cx);
                            wb.open_file(path, Some(at), window, cx);
                        });
                    }
                    None => {
                        wb.log(format!("{short}: not found in the workspace (framework or library code)."));
                        wb.show_panel(PanelView::Output, cx);
                    }
                }
            });
        }));
    }

    // ───────────── drawing ─────────────

    pub(super) fn render_profiler(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = Theme::global(cx);
        let muted = t.c("foreground.muted");
        let has_trace = profiling::global_tool(profiling::TRACE_TOOL).is_some();
        let has_gcdump = profiling::global_tool(profiling::GCDUMP_TOOL).is_some();
        let target = self.profiler_target();
        let recording = self.profiler.recording.as_ref().map(|r| (r.pid, r.name.clone(), r.started.elapsed().as_secs(), r.stopping));

        // The processes, the app being run first (shared with Live Metrics).
        let app_pid = self.metrics_app_pid();
        let mut processes = self.metrics_processes().to_vec();
        processes.sort_by_key(|p| (Some(p.pid) != app_pid, p.pid));
        let mut list = div().id("profiler-processes").flex_1().min_h_0().overflow_y_scroll().flex().flex_col();
        if processes.is_empty() {
            list = list.child(div().px_3().py_1().text_size(px(12.)).text_color(muted).child("No .NET process is running."));
        }
        for p in processes {
            let (pid, name) = (p.pid, p.name.clone());
            let selected = target.as_ref().is_some_and(|(t, _)| *t == pid);
            let label = if Some(pid) == app_pid { format!("{name}  (running here)") } else { name.clone() };
            list = list.child(
                div()
                    .id(SharedString::from(format!("profiler-proc-{pid}")))
                    .debug_selector(move || format!("profiler-proc-{pid}"))
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
                    .on_click(cx.listener(move |wb, _, _, cx| wb.pick_profiler_target(pid, name.clone(), cx))),
            );
        }
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
        let left = div()
            .w(px(240.))
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
                    .child(small("profiler-refresh", "refresh", "Refresh").on_click(cx.listener(|wb, _, _, cx| wb.list_metrics_processes(cx)))),
            )
            .child(list);

        // The toolbar: record / stop, snapshot, the views, the options.
        let button = |id: &'static str, icon_name: &'static str, label: String, enabled: bool| {
            div()
                .id(id)
                .debug_selector(move || id.to_owned())
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .py(px(2.))
                .rounded(px(3.))
                .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(t.c("toolbar.hoverBackground"))))
                .when(!enabled, |d| d.opacity(0.5))
                .child(icon(icon_name, px(14.), t.c("icon.foreground")))
                .child(label)
        };
        let busy = self.profiler.busy.as_ref().map(|(text, _)| text.clone());
        let can_act = target.is_some() && busy.is_none();
        let mut toolbar = div().px_2().py_1().flex().items_center().gap_1().flex_wrap().border_b_1().border_color(t.c("panel.border")).text_size(px(12.));
        toolbar = match &recording {
            Some((_, _, _, stopping)) => toolbar.child(
                button("profiler-stop", "stop", if *stopping { "Finishing…".into() } else { "Stop".into() }, !stopping)
                    .when(!stopping, |d| d.on_click(cx.listener(|wb, _, _, cx| wb.stop_cpu_profile(cx)))),
            ),
            None => toolbar.child(
                button("profiler-record", "record", "Record CPU Profile".into(), can_act && has_trace).when(can_act && has_trace, |d| {
                    d.on_click(cx.listener(|wb, _, _, cx| wb.start_cpu_profile(cx)))
                }),
            ),
        };
        toolbar = toolbar.child(
            button("profiler-snapshot", "memory", "Memory Snapshot".into(), can_act && has_gcdump && recording.is_none())
                .when(can_act && has_gcdump && recording.is_none(), |d| d.on_click(cx.listener(|wb, _, _, cx| wb.take_memory_snapshot(cx)))),
        );
        toolbar = toolbar.child(div().w(px(12.)));
        let view = self.profiler.view;
        for (v, id, label) in [
            (ProfilerView::HotMethods, "profiler-view-hot", "Hot Methods"),
            (ProfilerView::CallTree, "profiler-view-tree", "Call Tree"),
            (ProfilerView::FlameGraph, "profiler-view-flame", "Flame Graph"),
            (ProfilerView::Memory, "profiler-view-memory", "Memory"),
        ] {
            toolbar = toolbar.child(
                div()
                    .id(id)
                    .debug_selector(move || id.to_owned())
                    .px_2()
                    .py(px(2.))
                    .rounded(px(3.))
                    .cursor_pointer()
                    .when(view == v, |d| d.bg(t.c("list.activeSelectionBackground")))
                    .when(view != v, |d| d.hover(|s| s.bg(t.c("toolbar.hoverBackground"))))
                    .child(label)
                    .on_click(cx.listener(move |wb, _, _, cx| {
                        wb.profiler.view = v;
                        cx.notify();
                    })),
            );
        }
        toolbar = toolbar.child(div().flex_1());
        let check = |id: &'static str, on: bool, label: &'static str, tip: &'static str| {
            div()
                .id(id)
                .debug_selector(move || id.to_owned())
                .flex()
                .items_center()
                .gap_1()
                .px_1()
                .cursor_pointer()
                .tooltip(tooltip(tip))
                .child(div().size(px(12.)).rounded(px(2.)).border_1().border_color(t.c("checkbox.border")).when(on, |d| d.bg(t.c("focusBorder"))))
                .child(label)
        };
        if view != ProfilerView::Memory {
            toolbar = toolbar.child(check("profiler-all-time", self.profiler.all_time, "Include waiting", "Count time the threads waited too, not only time running .NET code").on_click(
                cx.listener(|wb, _, _, cx| {
                    wb.profiler.all_time = !wb.profiler.all_time;
                    cx.notify();
                }),
            ));
        }
        toolbar = toolbar.child(check("profiler-my-code", self.profiler.my_code, "Only my code", "Only methods and types of this workspace's projects").on_click(
            cx.listener(|wb, _, _, cx| {
                wb.profiler.my_code = !wb.profiler.my_code;
                cx.notify();
            }),
        ));

        // Status: what's shown, recording, busy, a note.
        let status: Option<String> = if let Some((pid, name, secs, stopping)) = &recording {
            Some(if *stopping {
                format!("Finishing the trace of {name} ({pid}): dotnet-trace resolves the method names…")
            } else {
                format!("Recording {name} ({pid}) for {secs} s. Stop to see where the time went.")
            })
        } else if let Some(b) = busy {
            Some(b)
        } else if let Some(note) = &self.profiler.note {
            Some(note.clone())
        } else if target.is_none() {
            Some("Pick a process on the left, or run the app: its process is picked by itself.".into())
        } else {
            None
        };

        let mut right = div().flex_1().min_w_0().flex().flex_col().overflow_hidden().child(toolbar);
        if !has_trace || !has_gcdump {
            let missing: Vec<&str> = [(has_trace, "dotnet-trace"), (has_gcdump, "dotnet-gcdump")].into_iter().filter(|(has, _)| !has).map(|(_, n)| n).collect();
            right = right.child(
                div()
                    .px_3()
                    .py_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.))
                    .child(div().text_color(muted).child(format!("Profiling uses {}, .NET global tools that aren't installed.", missing.join(" and "))))
                    .child(
                        div()
                            .id("profiler-install")
                            .debug_selector(|| "profiler-install".into())
                            .px_2()
                            .py(px(2.))
                            .rounded(px(3.))
                            .cursor_pointer()
                            .bg(t.c("button.background"))
                            .text_color(t.c("button.foreground"))
                            .child("Install")
                            .on_click(cx.listener(|wb, _, w, cx| wb.install_profiling_tools(w, cx))),
                    ),
            );
        }
        if let Some(status) = status {
            right = right.child(div().px_3().py_1().text_size(px(12.)).text_color(muted).child(status));
        }
        let body = match view {
            ProfilerView::Memory => self.render_heap(cx),
            _ => match self.profiler.profile.clone() {
                Some(profile) => {
                    let header = div().px_3().py(px(2.)).text_size(px(11.)).text_color(muted).child(self.profiler.profile_of.clone());
                    let content = match view {
                        ProfilerView::HotMethods => self.render_hot_methods(&profile, cx),
                        ProfilerView::CallTree => self.render_call_tree(&profile, cx),
                        _ => self.render_flame_graph(&profile, cx),
                    };
                    div().flex_1().min_h_0().flex().flex_col().child(header).child(content).into_any_element()
                }
                None => div()
                    .p_3()
                    .text_color(muted)
                    .text_size(px(12.))
                    .child("Record a CPU profile of the app while it does the slow thing, then stop: the methods it spent the time in show up here.")
                    .into_any_element(),
            },
        };
        right = right.child(body);
        div().size_full().flex().text_size(px(12.)).child(left).child(right).into_any_element()
    }

    /// The share of the whole a time is, for the columns.
    fn pct(part: f64, whole: f64) -> String {
        if whole <= 0.0 { "–".into() } else { format!("{:.1}%", part * 100.0 / whole) }
    }

    fn ms(v: f64) -> String {
        if v >= 10_000.0 { format!("{:.1} s", v / 1000.0) } else { format!("{v:.0} ms") }
    }

    /// The Hot Methods rows: by own time, most first (only the workspace's methods with Only my code).
    fn hot_rows<'a>(&self, profile: &'a Profile) -> Vec<&'a profiling::MethodTimes> {
        let cpu = !self.profiler.all_time;
        let mut methods: Vec<&profiling::MethodTimes> = profile
            .methods
            .iter()
            .filter(|m| m.total.get(cpu) > 0.0)
            .filter(|m| !self.profiler.my_code || self.profiler.is_user(&profile.frames[m.frame].module))
            .collect();
        methods.sort_by(|a, b| b.own.get(cpu).total_cmp(&a.own.get(cpu)).then(b.total.get(cpu).total_cmp(&a.total.get(cpu))));
        methods
    }

    /// The Call Tree rows that are open, depth first: (node, depth).
    fn tree_rows(&self, profile: &Profile) -> Vec<(usize, usize)> {
        let cpu = !self.profiler.all_time;
        let mut visible: Vec<(usize, usize)> = Vec::new();
        let mut stack: Vec<(usize, usize)> = profile.children_sorted(0, cpu).into_iter().rev().map(|n| (n, 0)).collect();
        while let Some((node, depth)) = stack.pop() {
            if profile.nodes[node].total.get(cpu) <= 0.0 {
                continue;
            }
            visible.push((node, depth));
            if visible.len() >= MAX_ROWS * 4 {
                break;
            }
            if self.profiler.expanded.contains(&node) {
                for c in profile.children_sorted(node, cpu).into_iter().rev() {
                    stack.push((c, depth + 1));
                }
            }
        }
        visible
    }

    fn render_hot_methods(&mut self, profile: &Arc<Profile>, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = Theme::global(cx);
        let muted = t.c("foreground.muted");
        let cpu = !self.profiler.all_time;
        let whole = profile.root().total.get(cpu);
        let methods = self.hot_rows(profile);
        let header = div()
            .px_3()
            .py(px(2.))
            .flex()
            .gap_2()
            .text_size(px(11.))
            .text_color(muted)
            .child(div().w(px(130.)).flex_none().child("SELF"))
            .child(div().w(px(130.)).flex_none().child("TOTAL"))
            .child(div().flex_1().child("METHOD"));
        let mut rows = div().id("profiler-hot").flex_1().min_h_0().overflow_y_scroll().flex().flex_col();
        if methods.is_empty() {
            rows = rows.child(div().px_3().py_1().text_color(muted).child(if self.profiler.my_code { "None of the time was in this workspace's code." } else { "No samples in this trace." }));
        }
        let bar = t.c_or("profiler.barBackground", 0x3794ff55);
        for (i, m) in methods.into_iter().take(MAX_ROWS).enumerate() {
            let f = &profile.frames[m.frame];
            let frame = m.frame;
            let own_share = if whole > 0.0 { (m.own.get(cpu) / whole) as f32 } else { 0.0 };
            let user = self.profiler.is_user(&f.module);
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("profiler-hot-{i}")))
                    .debug_selector(move || format!("profiler-hot-{i}"))
                    .px_3()
                    .py(px(1.))
                    .flex()
                    .gap_2()
                    .cursor_pointer()
                    .hover(|s| s.bg(t.c("list.hoverBackground")))
                    .tooltip(tooltip(format!("{}\nClick: open the source", f.name)))
                    .child(
                        div()
                            .w(px(130.))
                            .flex_none()
                            .relative()
                            .child(div().absolute().top_0().bottom_0().left_0().w(px(130. * own_share.clamp(0.0, 1.0))).bg(bar))
                            .child(div().relative().child(format!("{}  {}", Self::ms(m.own.get(cpu)), Self::pct(m.own.get(cpu), whole)))),
                    )
                    .child(div().w(px(130.)).flex_none().text_color(muted).child(format!("{}  {}", Self::ms(m.total.get(cpu)), Self::pct(m.total.get(cpu), whole))))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .gap_2()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(div().when(!user, |d| d.text_color(muted)).child(f.short()))
                            .child(div().text_color(muted).child(f.module.clone())),
                    )
                    .on_click(cx.listener(move |wb, _, _, cx| wb.open_profiled_method(frame, cx))),
            );
        }
        div().flex_1().min_h_0().flex().flex_col().child(header).child(rows).into_any_element()
    }

    fn render_call_tree(&mut self, profile: &Arc<Profile>, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = Theme::global(cx);
        let muted = t.c("foreground.muted");
        let cpu = !self.profiler.all_time;
        let whole = profile.root().total.get(cpu);
        let visible = self.tree_rows(profile);
        let header = div()
            .px_3()
            .py(px(2.))
            .flex()
            .gap_2()
            .text_size(px(11.))
            .text_color(muted)
            .child(div().w(px(120.)).flex_none().child("TOTAL"))
            .child(div().w(px(110.)).flex_none().child("SELF"))
            .child(div().flex_1().child("METHOD"));
        let mut rows = div().id("profiler-tree").flex_1().min_h_0().overflow_y_scroll().flex().flex_col();
        for (node, depth) in visible {
            let n = &profile.nodes[node];
            let Some(f) = profile.frame(n) else { continue };
            let open = self.profiler.expanded.contains(&node);
            let has_children = n.children.iter().any(|&c| profile.nodes[c].total.get(cpu) > 0.0);
            let user = self.profiler.is_user(&f.module);
            let frame = n.frame;
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("profiler-node-{node}")))
                    .debug_selector(move || format!("profiler-node-{node}"))
                    .px_3()
                    .py(px(1.))
                    .flex()
                    .gap_2()
                    .cursor_pointer()
                    .hover(|s| s.bg(t.c("list.hoverBackground")))
                    .tooltip(tooltip(format!("{}\nClick: open or close · Double-click: open the source", f.name)))
                    .child(div().w(px(120.)).flex_none().child(format!("{}  {}", Self::pct(n.total.get(cpu), whole), Self::ms(n.total.get(cpu)))))
                    .child(div().w(px(110.)).flex_none().text_color(muted).child(if n.own.get(cpu) > 0.0 { Self::ms(n.own.get(cpu)) } else { String::new() }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(div().flex_none().w(px(depth as f32 * 14.)))
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(14.))
                                    .when(has_children, |d| d.child(icon(if open { "chevronDown" } else { "chevronRight" }, px(12.), t.c("icon.foreground")))),
                            )
                            .child(div().when(!user, |d| d.text_color(muted)).child(f.short()))
                            .child(div().text_color(muted).child(f.module.clone())),
                    )
                    .on_click(cx.listener(move |wb, e: &gpui::ClickEvent, _, cx| {
                        if e.click_count() >= 2 {
                            wb.open_profiled_method(frame, cx);
                        } else if has_children {
                            if !wb.profiler.expanded.remove(&node) {
                                wb.profiler.expanded.insert(node);
                            }
                            cx.notify();
                        }
                    })),
            );
        }
        div().flex_1().min_h_0().flex().flex_col().child(header).child(rows).into_any_element()
    }

    /// An icicle chart: the zoomed node (all threads at first) on top, what it called below, each as wide as
    /// its share of the time. Click: zoom in (the rows above zoom back out); double-click: open the source.
    fn render_flame_graph(&mut self, profile: &Arc<Profile>, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = Theme::global(cx);
        let muted = t.c("foreground.muted");
        let cpu = !self.profiler.all_time;
        let whole = profile.root().total.get(cpu);
        let zoom = self.profiler.zoom.filter(|&z| z < profile.nodes.len()).unwrap_or(0);
        let user_color = t.c_or("profiler.flame.userCode", 0xe0913aff);
        let other_color = t.c_or("profiler.flame.otherCode", 0x7a8699ff);
        let text_color = t.c_or("profiler.flame.foreground", 0x1b1b1bff);
        // The rows above: the zoomed node's callers, back to "all threads".
        let mut chain = vec![zoom];
        while let Some(&last) = chain.last() {
            if last == 0 {
                break;
            }
            chain.push(self.profiler.parents.get(last).copied().unwrap_or(0));
        }
        chain.reverse();
        let mut bars: Vec<(usize, usize, f32, f32)> = chain.iter().enumerate().map(|(depth, &n)| (n, depth, 0.0, 1.0)).collect();
        let base_depth = chain.len() - 1;
        let zoom_total = profile.nodes[zoom].total.get(cpu).max(f64::MIN_POSITIVE);
        // Below the zoomed node: children laid out left to right by time.
        let mut stack = vec![(zoom, base_depth, 0.0f32, 1.0f32)];
        while let Some((node, depth, x, w)) = stack.pop() {
            let mut cx_ = x;
            for c in profile.children_sorted(node, cpu) {
                let share = (profile.nodes[c].total.get(cpu) / zoom_total) as f32;
                if share < 0.002 {
                    continue;
                }
                bars.push((c, depth + 1, cx_, share));
                stack.push((c, depth + 1, cx_, share));
                cx_ += share;
            }
            let _ = w;
        }
        let depth_max = bars.iter().map(|b| b.1).max().unwrap_or(0);
        let mut canvas = div().relative().w_full().h(px((depth_max + 1) as f32 * FLAME_ROW));
        for (node, depth, x, w) in bars {
            let n = &profile.nodes[node];
            let (label, user, frame) = match profile.frame(n) {
                Some(f) => (f.short(), self.profiler.is_user(&f.module), Some(n.frame)),
                None => ("All threads".to_owned(), false, None),
            };
            let above = depth < base_depth;
            let color = if user { user_color } else { other_color };
            let tip = match profile.frame(n) {
                Some(f) => format!("{}\n{}  {}\nClick: zoom in · Double-click: open the source", f.name, Self::ms(n.total.get(cpu)), Self::pct(n.total.get(cpu), whole)),
                None => format!("All threads: {}", Self::ms(n.total.get(cpu))),
            };
            canvas = canvas.child(
                div()
                    .id(SharedString::from(format!("profiler-flame-{node}")))
                    .debug_selector(move || format!("profiler-flame-{node}"))
                    .absolute()
                    .top(px(depth as f32 * FLAME_ROW))
                    .left(gpui::relative(x))
                    .w(gpui::relative(w))
                    .h(px(FLAME_ROW - 1.))
                    .pr(px(1.))
                    .child(
                        div()
                            .size_full()
                            .px_1()
                            .rounded(px(2.))
                            .bg(color)
                            .when(above, |d| d.opacity(0.55))
                            .text_color(text_color)
                            .text_size(px(11.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .cursor_pointer()
                            .hover(|s| s.opacity(0.85))
                            .child(label),
                    )
                    .tooltip(tooltip(tip))
                    .on_click(cx.listener(move |wb, e: &gpui::ClickEvent, _, cx| {
                        if e.click_count() >= 2 {
                            if let Some(frame) = frame {
                                wb.open_profiled_method(frame, cx);
                            }
                        } else {
                            wb.profiler.zoom = Some(node);
                            cx.notify();
                        }
                    })),
            );
        }
        div()
            .id("profiler-flame")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_3()
            .py_1()
            .child(canvas)
            .child(div().pt_1().text_size(px(11.)).text_color(muted).child("Click a frame to zoom in, a row above to zoom back out."))
            .into_any_element()
    }

    fn render_heap(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = Theme::global(cx);
        let muted = t.c("foreground.muted");
        let Some(heap) = self.profiler.heap.as_ref() else {
            return div()
                .p_3()
                .text_color(muted)
                .text_size(px(12.))
                .child("Take a memory snapshot to see what's on the heap. Take another later: the change shows what grew (a leak keeps growing).")
                .into_any_element();
        };
        let before = self.profiler.heap_before.as_ref();
        let fmt = |b: f64| bf_core::metrics::format_bytes(b);
        let mut summary = format!("{} · {} in {} objects", self.profiler.heap_of, fmt(heap.total_bytes as f64), group(heap.total_objects));
        if let Some(b) = before {
            let delta = heap.total_bytes as i64 - b.total_bytes as i64;
            summary.push_str(&format!(" · {} since the snapshot before", signed(delta, fmt)));
        }
        let header = div()
            .px_3()
            .py(px(2.))
            .flex()
            .gap_2()
            .text_size(px(11.))
            .text_color(muted)
            .child(div().w(px(90.)).flex_none().child("SIZE"))
            .child(div().w(px(80.)).flex_none().child("COUNT"))
            .when(before.is_some(), |d| d.child(div().w(px(150.)).flex_none().child("CHANGE")))
            .child(div().flex_1().child("TYPE"));
        let grew = t.c_or("profiler.grew", 0xf14c4cff);
        let shrank = t.c_or("profiler.shrank", 0x89d185ff);
        let mut rows = div().id("profiler-heap").flex_1().min_h_0().overflow_y_scroll().flex().flex_col();
        let types: Vec<&profiling::HeapType> = heap.types.iter().filter(|ty| !self.profiler.my_code || self.profiler.is_user(&ty.module)).take(MAX_ROWS).collect();
        for (i, ty) in types.into_iter().enumerate() {
            let user = self.profiler.is_user(&ty.module);
            let change = before.map(|b| {
                let old = b.get(&ty.name);
                let bytes = ty.bytes as i64 - old.map_or(0, |o| o.bytes as i64);
                let count = ty.count as i64 - old.map_or(0, |o| o.count as i64);
                (bytes, count)
            });
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("profiler-type-{i}")))
                    .debug_selector(move || format!("profiler-type-{i}"))
                    .px_3()
                    .py(px(1.))
                    .flex()
                    .gap_2()
                    .hover(|s| s.bg(t.c("list.hoverBackground")))
                    .child(div().w(px(90.)).flex_none().child(fmt(ty.bytes as f64)))
                    .child(div().w(px(80.)).flex_none().text_color(muted).child(group(ty.count)))
                    .when_some(change, |d, (bytes, count)| {
                        let color = if bytes > 0 { grew } else if bytes < 0 { shrank } else { muted };
                        let text = if bytes == 0 && count == 0 { "–".to_owned() } else { format!("{} ({}{})", signed(bytes, fmt), if count >= 0 { "+" } else { "" }, count) };
                        d.child(div().w(px(150.)).flex_none().text_color(color).child(text))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .gap_2()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(div().when(!user, |d| d.text_color(muted)).child(ty.name.clone()))
                            .child(div().text_color(muted).child(ty.module.clone())),
                    ),
            );
        }
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(div().px_3().py(px(2.)).text_size(px(11.)).text_color(muted).child(summary))
            .child(header)
            .child(rows)
            .into_any_element()
    }
}

/// 43278 → "43,278".
fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A byte change with its sign: "+1.2 MB", "−40 KB".
fn signed(bytes: i64, fmt: impl Fn(f64) -> String) -> String {
    match bytes {
        0 => "±0".into(),
        b if b > 0 => format!("+{}", fmt(b as f64)),
        b => format!("−{}", fmt((-b) as f64)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workbench::settings_page::view_tests::workbench;

    #[test]
    fn numbers() {
        assert_eq!(group(43_278), "43,278");
        assert_eq!(group(7), "7");
        assert_eq!(group(1_000_000), "1,000,000");
        assert_eq!(signed(0, |b| format!("{b}")), "±0");
        assert_eq!(signed(-5, |b| format!("{b}")), "−5");
    }

    const TRACE: &str = r#"{"shared":{"frames":[
        {"name":"Thread (7)"},{"name":"Busy!Program.Main()"},{"name":"Busy!Work.Slow(int32)"},{"name":"System.Private.CoreLib.il!System.Math.Sqrt(float64)"},{"name":"CPU_TIME"}]},
      "profiles":[{"type":"evented","startValue":0,"endValue":100,"events":[
        {"type":"O","frame":0,"at":0},{"type":"O","frame":1,"at":0},{"type":"O","frame":2,"at":0},{"type":"O","frame":4,"at":0},
        {"type":"C","frame":4,"at":30},{"type":"O","frame":3,"at":30},{"type":"O","frame":4,"at":30},
        {"type":"C","frame":4,"at":100},{"type":"C","frame":3,"at":100},{"type":"C","frame":2,"at":100},{"type":"C","frame":1,"at":100},{"type":"C","frame":0,"at":100}]}]}"#;

    /// A trace read back shows in every view; Only my code hides the framework's method; the flame graph
    /// zooms in and back out; a heap snapshot shows its types and, with a second one, what changed.
    #[gpui::test]
    fn the_profiler_shows_a_trace_and_heap_snapshots(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "profiler");
        wb.update(cx, |wb, cx| {
            let profile = profiling::parse_speedscope(TRACE).unwrap();
            wb.profiler.profile_of = "Busy (1) · 0.1 s".into();
            wb.set_profile(profile, HashSet::from(["busy".to_owned()]));
            wb.show_panel(PanelView::Profiler, cx);
        });
        cx.run_until_parked();
        let click = |cx: &mut gpui::VisualTestContext, id: &str| {
            let id: &'static str = Box::leak(id.to_owned().into_boxed_str());
            cx.update(|window, cx| window.draw(cx).clear());
            let at = cx.debug_bounds(id).unwrap_or_else(|| panic!("{id} is drawn")).center();
            cx.simulate_click(at, gpui::Modifiers::none());
            cx.run_until_parked();
        };
        let drawn = |cx: &mut gpui::VisualTestContext, id: &'static str| {
            cx.run_until_parked();
            cx.update(|window, cx| {
                window.refresh();
                window.draw(cx).clear()
            });
            cx.debug_bounds(id).is_some()
        };
        // Hot methods: Math.Sqrt (70 ms own) first, then Work.Slow.
        let hot = |wb: &Entity<Workbench>, cx: &mut gpui::VisualTestContext| {
            wb.read_with(cx, |wb, _| {
                let p = wb.profiler.profile.clone().unwrap();
                wb.hot_rows(&p).iter().map(|m| p.frames[m.frame].short()).collect::<Vec<_>>()
            })
        };
        assert!(drawn(cx, "profiler-hot-0") && drawn(cx, "profiler-hot-2"));
        assert_eq!(hot(&wb, cx), ["Math.Sqrt", "Work.Slow", "Program.Main"]);
        // Only my code (clicked): Sqrt goes.
        click(cx, "profiler-my-code");
        assert_eq!(hot(&wb, cx), ["Work.Slow", "Program.Main"]);
        // The call tree opens along the hot path: Main → Slow → Sqrt.
        click(cx, "profiler-view-tree");
        let (main, slow, sqrt) = wb.read_with(cx, |wb, _| {
            let p = wb.profiler.profile.clone().unwrap();
            let main = p.children_sorted(0, true)[0];
            let slow = p.children_sorted(main, true)[0];
            (main, slow, p.children_sorted(slow, true)[0])
        });
        for node in [main, slow, sqrt] {
            let id: &'static str = Box::leak(format!("profiler-node-{node}").into_boxed_str());
            assert!(drawn(cx, id), "{id}");
        }
        let tree = |wb: &Entity<Workbench>, cx: &mut gpui::VisualTestContext| {
            wb.read_with(cx, |wb, _| wb.tree_rows(&wb.profiler.profile.clone().unwrap()).into_iter().map(|r| r.0).collect::<Vec<_>>())
        };
        assert_eq!(tree(&wb, cx), [main, slow, sqrt]);
        // Clicking Slow closes it: Sqrt is hidden.
        click(cx, &format!("profiler-node-{slow}"));
        assert_eq!(tree(&wb, cx), [main, slow]);
        // Flame graph: zoom into Slow, then back out from the row above.
        click(cx, "profiler-view-flame");
        assert!(drawn(cx, "profiler-flame-0"));
        let slow_id: &'static str = Box::leak(format!("profiler-flame-{slow}").into_boxed_str());
        let before = cx.debug_bounds(slow_id).unwrap();
        cx.simulate_click(before.center(), gpui::Modifiers::none());
        assert_eq!(wb.read_with(cx, |wb, _| wb.profiler.zoom), Some(slow));
        cx.update(|window, cx| window.draw(cx).clear());
        let zoomed = cx.debug_bounds(slow_id).unwrap();
        assert!(zoomed.size.width >= before.size.width, "Slow fills the width now");
        click(cx, "profiler-flame-0");
        assert_eq!(wb.read_with(cx, |wb, _| wb.profiler.zoom), Some(0));

        // Memory: two snapshots of the same process; the second shows the change.
        wb.update(cx, |wb, cx| {
            let first = profiling::parse_gcdump_report("  1,000  GC Heap bytes\n  10  GC Heap objects\n   32   10  Order  [Busy.dll]\n");
            wb.set_heap(first, HashSet::new(), "Busy (1)".into());
            let second = profiling::parse_gcdump_report("  2,000  GC Heap bytes\n  40  GC Heap objects\n   32   40  Order  [Busy.dll]\n   24   1  Cache  [Busy.dll]\n");
            wb.set_heap(second, HashSet::new(), "Busy (1)".into());
            assert_eq!(wb.profiler.view, ProfilerView::Memory);
            assert!(wb.profiler.heap_before.is_some());
            cx.notify();
        });
        assert!(drawn(cx, "profiler-type-0") && drawn(cx, "profiler-type-1"));
        // A snapshot of another process isn't compared with that one.
        wb.update(cx, |wb, _| {
            wb.set_heap(HeapReport::default(), HashSet::new(), "Other (2)".into());
            assert!(wb.profiler.heap_before.is_none());
        });
        drop(guard);
    }

    /// Polls until `done`, at most `secs` seconds (real processes run).
    fn wait_for(cx: &mut gpui::VisualTestContext, wb: &Entity<Workbench>, secs: u64, mut done: impl FnMut(&Workbench) -> bool) {
        for _ in 0..secs * 10 {
            cx.run_until_parked();
            if wb.read_with(cx, |wb, _| done(wb)) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("timed out; note: {:?}", wb.read_with(cx, |wb, _| wb.profiler.note.clone()));
    }

    /// A real app profiled: dotnet-trace runs on its own terminal and stops on Enter, the trace shows the
    /// app's busy method; two memory snapshots show its objects and what grew. Needs the .NET SDK,
    /// dotnet-trace and dotnet-gcdump.
    #[gpui::test]
    #[ignore = "profiles a real process (about 40 seconds)"]
    fn profiling_a_real_process(cx: &mut gpui::TestAppContext) {
        if profiling::global_tool(profiling::TRACE_TOOL).is_none() || profiling::global_tool(profiling::GCDUMP_TOOL).is_none() {
            eprintln!("skipped: dotnet-trace or dotnet-gcdump isn't installed");
            return;
        }
        let (guard, wb, cx) = workbench(cx, "profile-real");
        let ws = guard.dir.join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let ok = std::process::Command::new("dotnet").args(["new", "console", "-o", "Busy", "-n", "Busy"]).current_dir(&ws).output().is_ok_and(|o| o.status.success());
        if !ok {
            eprintln!("skipped: dotnet isn't available");
            return;
        }
        std::fs::write(
            ws.join("Busy").join("Program.cs"),
            "var keep = new List<Order>();\nvar end = DateTime.UtcNow.AddSeconds(90);\nwhile (DateTime.UtcNow < end)\n{\n    keep.Add(new Order(keep.Count, Work.Slow(50_000)));\n}\n\nrecord Order(int Id, double Total);\n\nstatic class Work\n{\n    public static double Slow(int n)\n    {\n        double s = 0;\n        for (int i = 1; i < n; i++) s += Math.Sqrt(i) * Math.Sin(i);\n        return s;\n    }\n}\n",
        )
        .unwrap();
        assert!(std::process::Command::new("dotnet").args(["build", "Busy"]).current_dir(&ws).output().unwrap().status.success());
        let dll = std::fs::read_dir(ws.join("Busy").join("bin").join("Debug")).unwrap().flatten().next().unwrap().path().join("Busy.dll");
        let mut app = std::process::Command::new("dotnet").arg(&dll).spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_secs(2));
        wb.update_in(cx, |wb, window, cx| {
            wb.open_folder(ws.clone(), window, cx);
            wb.pick_profiler_target(app.id(), "Busy".into(), cx);
            wb.start_cpu_profile(cx);
        });
        wait_for(cx, &wb, 10, |wb| wb.profiler.recording.is_some());
        std::thread::sleep(std::time::Duration::from_secs(4));
        wb.update(cx, |wb, cx| wb.stop_cpu_profile(cx));
        wait_for(cx, &wb, 90, |wb| wb.profiler.profile.is_some() || (wb.profiler.recording.is_none() && wb.profiler.busy.is_none()));
        let hot = wb.read_with(cx, |wb, _| {
            let p = wb.profiler.profile.clone().unwrap_or_else(|| panic!("no profile: {:?}", wb.profiler.note));
            assert!(p.duration_ms < 30_000.0, "Enter stopped it ({} ms)", p.duration_ms);
            wb.hot_rows(&p).iter().take(5).map(|m| p.frames[m.frame].short()).collect::<Vec<_>>()
        });
        assert!(hot.iter().any(|m| m == "Work.Slow"), "{hot:?}");

        // Two snapshots: Orders grew in between.
        wb.update(cx, |wb, cx| wb.take_memory_snapshot(cx));
        wait_for(cx, &wb, 60, |wb| wb.profiler.busy.is_none());
        std::thread::sleep(std::time::Duration::from_secs(2));
        wb.update(cx, |wb, cx| wb.take_memory_snapshot(cx));
        wait_for(cx, &wb, 60, |wb| wb.profiler.busy.is_none());
        let _ = app.kill();
        let _ = app.wait();
        wb.read_with(cx, |wb, _| {
            let heap = wb.profiler.heap.as_ref().unwrap_or_else(|| panic!("no snapshot: {:?}", wb.profiler.note));
            let before = wb.profiler.heap_before.as_ref().expect("the snapshot before");
            let (now, then) = (heap.get("Order").expect("Orders on the heap"), before.get("Order").expect("Orders before"));
            assert!(now.count > then.count, "{} > {}", now.count, then.count);
        });
        drop(guard);
    }
}
