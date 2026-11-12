//! Debugging .NET programs with netcoredbg over the Debug Adapter Protocol: launching (build, then run
//! the built assembly with its launch profile, so ASP.NET Core, Razor Pages, MVC and Blazor Server apps
//! get their URLs and environment), attaching to a running process, breakpoints (also in `.razor` and
//! `.cshtml` files), stepping, the Run and Debug view and the Debug Console.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use bf_core::debug::{LaunchProfile, launch_profile, listening_url, open_browser, target_assembly};
use bf_dap::{AdapterConfig, DapClient, DapEvent, EvaluateContext, Scope, SourceBreakpoint, StackFrame, Variable};

use super::*;
use crate::editor::Breakpoint;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Step {
    Over,
    Into,
    Out,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    /// `dotnet build` runs before the debugger starts.
    Building,
    Starting,
    Running,
    Stopped,
}

enum Target {
    Launch { project: PathBuf, program: PathBuf },
    Attach { pid: u32 },
    /// A process in a Docker container (its PID there), debugged by the netcoredbg put into it.
    Container { container: String, pid: u32 },
}

/// Source paths of a container session: breakpoints go out as short workspace-relative paths, stack
/// frames come back with the container's paths (see `bf_core::container`).
struct ContainerPaths {
    root: PathBuf,
    files: Arc<Vec<PathBuf>>,
}

pub(super) struct DebugSession {
    /// Tells this session's late callbacks apart from a newer session's.
    generation: u64,
    /// `None` while building.
    pub(super) client: Option<DapClient>,
    phase: Phase,
    name: String,
    pub(super) attached: bool,
    /// The adapter is configured (breakpoints may be sent).
    configured: bool,
    thread_id: Option<i64>,
    stop_reason: String,
    frames: Vec<StackFrame>,
    frame: usize,
    scopes: Vec<Scope>,
    /// Children of expanded scopes and variables, by variables reference.
    children: HashMap<i64, Vec<Variable>>,
    expanded: HashSet<i64>,
    /// Adapter breakpoint ids → (file, 0-based line), to apply `breakpoint` events.
    bp_ids: HashMap<i64, (PathBuf, usize)>,
    profile: Option<LaunchProfile>,
    browser_opened: bool,
    container: Option<ContainerPaths>,
    /// The adapter's reason for the last stop ("breakpoint", "step", "exception", …).
    stop_kind: String,
    /// How often each breakpoint with a hit count was hit (file, 0-based line).
    hits: HashMap<(PathBuf, usize), u64>,
    /// Watch expression → its value in the selected frame (or why it has none).
    watch_values: HashMap<String, String>,
    _tasks: Vec<Task<()>>,
}

impl DebugSession {
    fn new(generation: u64, name: String, phase: Phase) -> Self {
        Self {
            generation,
            client: None,
            phase,
            name,
            attached: false,
            configured: false,
            thread_id: None,
            stop_reason: String::new(),
            frames: Vec::new(),
            frame: 0,
            scopes: Vec::new(),
            children: HashMap::new(),
            expanded: HashSet::new(),
            bp_ids: HashMap::new(),
            profile: None,
            browser_opened: false,
            container: None,
            stop_kind: String::new(),
            hits: HashMap::new(),
            watch_values: HashMap::new(),
            _tasks: Vec::new(),
        }
    }

    /// Status bar text.
    pub(super) fn status_text(&self) -> String {
        match self.phase {
            Phase::Building => format!("Building {}…", self.name),
            Phase::Starting => format!("Starting {}…", self.name),
            Phase::Running => format!("Debugging {}", self.name),
            Phase::Stopped => format!("Paused: {}", self.name),
        }
    }

    fn clear_stop(&mut self) {
        self.frames.clear();
        self.frame = 0;
        self.scopes.clear();
        self.children.clear();
        self.expanded.clear();
    }
}

fn is_web_project(project: &Path) -> bool {
    std::fs::read_to_string(project).is_ok_and(|text| text.contains("Microsoft.NET.Sdk.Web") || text.contains("Microsoft.NET.Sdk.Razor"))
}

/// A breakpoint in the state file: a bare line number (older saves, plain breakpoints) or the line with
/// its options.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
enum SavedBreakpoint {
    Line(usize),
    WithOptions {
        line: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        condition: Option<String>,
        #[serde(default, rename = "hitCondition", skip_serializing_if = "Option::is_none")]
        hit_condition: Option<String>,
        #[serde(default, rename = "logMessage", skip_serializing_if = "Option::is_none")]
        log_message: Option<String>,
    },
}

impl SavedBreakpoint {
    fn from_breakpoint(b: &Breakpoint) -> Self {
        if b.condition.is_none() && b.hit_condition.is_none() && b.log_message.is_none() {
            SavedBreakpoint::Line(b.line)
        } else {
            SavedBreakpoint::WithOptions { line: b.line, condition: b.condition.clone(), hit_condition: b.hit_condition.clone(), log_message: b.log_message.clone() }
        }
    }

    fn into_breakpoint(self) -> Breakpoint {
        match self {
            SavedBreakpoint::Line(line) => Breakpoint::new(line),
            SavedBreakpoint::WithOptions { line, condition, hit_condition, log_message } => Breakpoint { condition, hit_condition, log_message, ..Breakpoint::new(line) },
        }
    }
}

/// What a breakpoint does besides stopping, for the Breakpoints list: "when x > 3", "hit >= 5", "log: …".
fn breakpoint_detail(b: &Breakpoint) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(c) = &b.condition {
        parts.push(format!("when {c}"));
    }
    if let Some(h) = &b.hit_condition {
        parts.push(format!("hit {h}"));
    }
    if let Some(m) = &b.log_message {
        parts.push(format!("log: {m}"));
    }
    (!parts.is_empty()).then(|| parts.join(" \u{00b7} "))
}

/// Which option of a breakpoint a command edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BreakpointOption {
    Condition,
    HitCount,
    LogMessage,
}

impl Workbench {
    fn debug_print(&self, text: impl Into<String>, cx: &mut Context<Self>) {
        let text = text.into();
        self.debug_console.update(cx, |c, cx| c.append(&text, cx));
    }

    fn same_session(&self, generation: u64) -> bool {
        self.debug.as_ref().is_some_and(|s| s.generation == generation)
    }

    // ───────────── breakpoints ─────────────

    fn breakpoints_key(&self) -> Option<String> {
        self.workspace.root().map(|root| format!("breakpoints:{}", root.display()))
    }

    /// Restores the opened folder's breakpoints (saved in the state file).
    pub(super) fn load_breakpoints(&mut self, cx: &mut Context<Self>) {
        self.breakpoints.clear();
        let saved = self.breakpoints_key().and_then(|key| self.state.get(&key));
        if let Some(map) = saved.and_then(|text| serde_json::from_str::<BTreeMap<String, Vec<SavedBreakpoint>>>(&text).ok()) {
            for (path, saved) in map {
                let mut list: Vec<Breakpoint> = saved.into_iter().map(SavedBreakpoint::into_breakpoint).collect();
                list.sort_by_key(|b| b.line);
                list.dedup_by_key(|b| b.line);
                if !list.is_empty() {
                    self.breakpoints.insert(PathBuf::from(path), list);
                }
            }
        }
        for tab in &self.tabs {
            let list = tab.path.as_ref().and_then(|p| self.breakpoints.get(p)).cloned().unwrap_or_default();
            tab.editor.update(cx, |e, cx| e.set_breakpoints(list, cx));
        }
    }

    fn save_breakpoints(&self) {
        let Some(key) = self.breakpoints_key() else { return };
        let map: BTreeMap<String, Vec<SavedBreakpoint>> =
            self.breakpoints.iter().map(|(path, list)| (path.to_string_lossy().into_owned(), list.iter().map(SavedBreakpoint::from_breakpoint).collect())).collect();
        let text = serde_json::to_string(&map).unwrap_or_default();
        self.state.set(&key, (!map.is_empty()).then_some(text.as_str()));
    }

    /// Turns on the breakpoint margin for debuggable files, and applies their breakpoints and the last
    /// build's diagnostics.
    pub(super) fn prepare_debuggable_editor(&self, editor: &Entity<EditorView>, path: &Path, cx: &mut Context<Self>) {
        let breakpoints = self.breakpoints.get(path).cloned().unwrap_or_default();
        let diagnostics: Vec<_> = self.build_problems.iter().filter(|(p, _)| p == path).map(|(_, d)| d.clone()).collect();
        let lsp_running = self.lsp.is_some();
        editor.update(cx, |e, cx| {
            if matches!(e.language, Some(bf_syntax::Language::CSharp | bf_syntax::Language::Razor)) {
                e.glyph_margin = true;
                e.set_breakpoints(breakpoints, cx);
            }
            let build_only = e.language != Some(bf_syntax::Language::CSharp) || !lsp_running;
            if build_only && !diagnostics.is_empty() {
                e.set_diagnostics(diagnostics, cx);
            }
        });
    }

    pub(super) fn on_editor_breakpoints_changed(&mut self, editor: &Entity<EditorView>, cx: &mut Context<Self>) {
        let (path, list) = {
            let e = editor.read(cx);
            (e.path.clone(), e.breakpoints.clone())
        };
        let Some(path) = path else { return };
        if list.is_empty() {
            self.breakpoints.remove(&path);
        } else {
            self.breakpoints.insert(path.clone(), list);
        }
        self.save_breakpoints();
        self.debug_send_breakpoints(&path, cx);
        cx.notify();
    }

    pub(super) fn remove_all_breakpoints(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = std::mem::take(&mut self.breakpoints).into_keys().collect();
        for tab in &self.tabs {
            tab.editor.update(cx, |e, cx| e.set_breakpoints(Vec::new(), cx));
        }
        self.save_breakpoints();
        for path in paths {
            self.debug_send_breakpoints(&path, cx);
        }
        cx.notify();
    }

    fn remove_breakpoint(&mut self, path: &Path, line: usize, cx: &mut Context<Self>) {
        if let Some(list) = self.breakpoints.get_mut(path) {
            list.retain(|b| b.line != line);
            if list.is_empty() {
                self.breakpoints.remove(path);
            }
        }
        self.sync_editor_breakpoints(path, cx);
        self.save_breakpoints();
        self.debug_send_breakpoints(path, cx);
        cx.notify();
    }

    fn sync_editor_breakpoints(&self, path: &Path, cx: &mut Context<Self>) {
        let list = self.breakpoints.get(path).cloned().unwrap_or_default();
        for tab in self.tabs.iter().filter(|t| t.path.as_deref() == Some(path)) {
            tab.editor.update(cx, |e, cx| e.set_breakpoints(list.clone(), cx));
        }
    }

    /// Sends one file's breakpoints to the debugger and records which ones it could bind.
    fn debug_send_breakpoints(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(session) = self.debug.as_ref().filter(|s| s.configured) else { return };
        let Some(client) = session.client.clone() else { return };
        let generation = session.generation;
        let list: Vec<Breakpoint> = self.breakpoints.get(path).cloned().unwrap_or_default();
        let lines: Vec<usize> = list.iter().map(|b| b.line).collect();
        // The condition is the debugger's; hit counts and log messages are handled when it stops.
        let request: Vec<SourceBreakpoint> = list.iter().map(|b| SourceBreakpoint { line: b.line as u32 + 1, condition: b.condition.clone() }).collect();
        // In a container the program was built elsewhere: netcoredbg matches a relative path by its end.
        let sent = match &session.container {
            Some(c) => PathBuf::from(bf_core::container::breakpoint_path(path, &c.root, &c.files)),
            None => path.to_path_buf(),
        };
        let rx = client.set_breakpoints(&sent, &request);
        let path = path.to_path_buf();
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.recv_async().await else { return };
            let _ = this.update(cx, |wb, cx| {
                let Some(session) = wb.debug.as_mut().filter(|s| s.generation == generation) else { return };
                match result {
                    Ok(bound) => {
                        session.bp_ids.retain(|_, (p, _)| *p != path);
                        if let Some(list) = wb.breakpoints.get_mut(&path) {
                            // The response lists the breakpoints in request order.
                            for (bp, line) in bound.iter().zip(&lines) {
                                if let Some(b) = list.iter_mut().find(|b| b.line == *line) {
                                    b.verified = bp.verified;
                                }
                                if let Some(id) = bp.id {
                                    session.bp_ids.insert(id, (path.clone(), *line));
                                }
                            }
                        }
                        wb.sync_editor_breakpoints(&path, cx);
                    }
                    Err(e) => wb.debug_print(format!("Could not set breakpoints in {}: {e:#}\n", path.display()), cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ───────────── starting and ending sessions ─────────────

    /// F5: builds the startup project and runs it under the debugger, or continues when paused.
    pub(super) fn debug_start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(session) = &self.debug {
            if session.phase == Phase::Stopped {
                self.debug_continue(cx);
            }
            return;
        }
        let Some(root) = self.workspace.root() else {
            self.log("Open a folder containing a .NET project first.");
            self.show_panel(PanelView::Output, cx);
            return;
        };
        let Some(adapter) = self.debug_adapter(cx) else { return };
        if self.dotnet_busy(cx) {
            return;
        }
        let configured = self.settings.get_string("dotnet.project");
        let project = find_target(&root, configured.as_deref(), true).filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("csproj")));
        let Some(project) = project else {
            self.log(format!("No .csproj to debug in {}. Set \"dotnet.project\" to the project to start.", root.display()));
            self.show_panel(PanelView::Output, cx);
            return;
        };
        self.save_all(window, cx);
        let name = project.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.debug_generation += 1;
        let generation = self.debug_generation;
        self.debug = Some(DebugSession::new(generation, name.clone(), Phase::Building));
        // The build runs in Output; the Debug Console is for the program and starts when it launches.
        self.debug_console.update(cx, |c, cx| c.reload_text("", cx));
        self.show_side(SideView::Debug, cx);
        self.show_panel(PanelView::Output, cx);
        let args = vec!["build".to_owned(), project.to_string_lossy().into_owned()];
        let then: DotnetDone = Box::new(move |wb: &mut Workbench, ok: bool, window: &mut Window, cx: &mut Context<Workbench>| {
            if !wb.same_session(generation) {
                return;
            }
            if ok {
                wb.debug_launch(generation, project, adapter, window, cx);
            } else {
                wb.log("The build failed, so debugging didn't start. Problems lists the errors.");
                wb.debug = None;
                wb.show_panel(PanelView::Problems, cx);
            }
        });
        if !self.start_dotnet(root, args, Some(then), window, cx) {
            self.debug = None;
        }
        cx.notify();
    }

    /// The netcoredbg executable, or logs how to install it.
    fn debug_adapter(&mut self, cx: &mut Context<Self>) -> Option<String> {
        let configured = self.settings.get_string("debug.netcoredbgPath");
        let found = bf_dap::netcoredbg_command(configured.as_deref());
        if found.is_none() {
            self.log(
                "The .NET debugger netcoredbg was not found. Install it (Arch: the AUR package \"netcoredbg\"; elsewhere: unpack a \
                 release from https://github.com/Samsung/netcoredbg/releases to ~/.local/share/netcoredbg), or set \"debug.netcoredbgPath\".",
            );
            self.show_panel(PanelView::Output, cx);
        }
        found
    }

    fn debug_launch(&mut self, generation: u64, project: PathBuf, adapter: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(session) = self.debug.as_mut() {
            session.phase = Phase::Starting;
        }
        self.show_panel(PanelView::DebugConsole, cx);
        let task = cx.background_spawn({
            let project = project.clone();
            async move { target_assembly(&project) }
        });
        cx.spawn_in(window, async move |this, cx| {
            let program = task.await;
            let _ = this.update_in(cx, |wb, window, cx| {
                if !wb.same_session(generation) {
                    return;
                }
                match program {
                    Ok(program) => wb.debug_connect(generation, Target::Launch { project, program }, adapter, window, cx),
                    Err(e) => {
                        wb.debug_print(format!("Could not find the program to debug: {e:#}\n"), cx);
                        wb.debug_end(cx);
                    }
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn debug_attach(&mut self, pid: u32, window: &mut Window, cx: &mut Context<Self>) {
        if self.debug.is_some() {
            self.log("Stop the current debug session first (Shift+F5).");
            self.show_panel(PanelView::Output, cx);
            return;
        }
        let Some(adapter) = self.debug_adapter(cx) else { return };
        self.debug_generation += 1;
        let generation = self.debug_generation;
        self.debug = Some(DebugSession::new(generation, format!("process {pid}"), Phase::Starting));
        self.debug_console.update(cx, |c, cx| c.reload_text("", cx));
        self.debug_print(format!("Attaching to process {pid}…\n"), cx);
        self.show_side(SideView::Debug, cx);
        self.show_panel(PanelView::DebugConsole, cx);
        self.debug_connect(generation, Target::Attach { pid }, adapter, window, cx);
    }

    pub(super) fn open_attach_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let task = cx.background_spawn(async { bf_core::debug::dotnet_processes() });
        cx.spawn_in(window, async move |this, cx| {
            let processes = task.await;
            let _ = this.update_in(cx, |wb, window, cx| {
                if processes.is_empty() {
                    wb.log("No running .NET processes found.");
                    wb.show_panel(PanelView::Output, cx);
                    return;
                }
                let items = processes
                    .into_iter()
                    .map(|p| {
                        let mut item = PaletteItem::new(format!("{}  ({})", p.name, p.pid), PaletteAction::Command(Box::new(AttachToPid { pid: p.pid })));
                        item.detail = Some(short(&p.command_line, 140));
                        item.search = Some(format!("{} {} {}", p.name, p.pid, p.command_line));
                        item
                    })
                    .collect();
                wb.menu_open = None;
                let palette = cx.new(|cx| Palette::new(Mode::Pick, items, "Select a .NET process to attach to", "", window, cx));
                let sub = cx.subscribe_in(&palette, window, Self::on_palette_event);
                palette.update(cx, |p, cx| p.focus_input(window, cx));
                wb.palette = Some((palette, sub));
                cx.notify();
            });
        })
        .detach();
    }

    // ───────────── containers ─────────────

    /// "Start Debugging in Container": builds the startup project's image from its Dockerfile (as Debug
    /// when the Dockerfile can), starts a container from it and attaches the debugger to the app in it.
    /// The container is removed when the session ends.
    pub(super) fn debug_start_in_container(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.debug.is_some() {
            self.log("Stop the current debug session first (Shift+F5).");
            self.show_panel(PanelView::Output, cx);
            return;
        }
        let Some(root) = self.workspace.root() else { return };
        let Some(project) = self.startup_project() else {
            self.log("No .csproj to run in this folder. Set \"dotnet.project\" to the project to start.");
            self.show_panel(PanelView::Output, cx);
            return;
        };
        let Some(plan) = bf_core::container::plan(&project, &root) else {
            self.log("Start Debugging in Container needs a Dockerfile next to the project or in the folder (the Docker module's \"Add Docker Support\" writes one).");
            self.show_panel(PanelView::Output, cx);
            return;
        };
        if self.dotnet_busy(cx) {
            return;
        }
        self.save_all(window, cx);
        self.show_panel(PanelView::Output, cx);
        if !plan.debug_build {
            self.log("The Dockerfile has no BUILD_CONFIGURATION argument, so the image is built as it says (probably Release): breakpoints may not bind as well as in a Debug build.");
        }
        let args = plan.build_args();
        let then: DotnetDone = Box::new(move |wb, ok, window, cx| {
            if !ok {
                wb.log("The image didn't build; see the output above.");
                wb.show_panel(PanelView::Output, cx);
                return;
            }
            let starting = plan.clone();
            let task = cx.background_spawn(async move {
                starting.start()?;
                // The app needs a moment to be up before its process shows.
                std::thread::sleep(std::time::Duration::from_millis(1200));
                let apps = bf_core::container::app_candidates(&bf_core::container::processes(&starting.name));
                anyhow::Ok((starting.ports(), apps.first().map(|p| p.pid)))
            });
            cx.spawn_in(window, async move |this, cx| {
                let started = task.await;
                let _ = this.update_in(cx, |wb, window, cx| match started {
                    Ok((ports, pid)) => {
                        wb.log(format!("Container {} is running.", plan.name));
                        for port in ports {
                            wb.log(format!("  port {port}"));
                        }
                        let name = plan.name.clone();
                        wb.debug_container = Some(plan);
                        wb.debug_attach_container(name, pid.unwrap_or(1), window, cx);
                    }
                    Err(e) => {
                        wb.log(format!("Could not start the container: {e:#}"));
                        wb.show_panel(PanelView::Output, cx);
                    }
                });
            })
            .detach();
        });
        self.start_program_with("docker", root, args, Vec::new(), None, Some(then), window, cx);
    }

    /// "Attach to .NET Process in Container…": the running containers (straight to the processes when
    /// there is only one).
    pub(super) fn open_container_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let task = cx.background_spawn(async { bf_core::container::containers() });
        cx.spawn_in(window, async move |this, cx| {
            let containers = task.await;
            let _ = this.update_in(cx, |wb, window, cx| match containers {
                Err(e) => {
                    wb.log(format!("Could not list containers: {e:#}"));
                    wb.show_panel(PanelView::Output, cx);
                }
                Ok(list) if list.is_empty() => {
                    wb.log("No running containers. Start the app's container first (docker run / docker compose up).");
                    wb.show_panel(PanelView::Output, cx);
                }
                Ok(list) if list.len() == 1 => wb.open_container_process_picker(list[0].name.clone(), window, cx),
                Ok(list) => {
                    let items = list
                        .into_iter()
                        .map(|c| {
                            let mut item = PaletteItem::new(c.name.clone(), PaletteAction::Command(Box::new(PickContainerProcess { container: c.name.clone() })));
                            item.detail = Some(format!("{} \u{2022} {}", c.image, c.status));
                            item.search = Some(format!("{} {} {}", c.name, c.image, c.id));
                            item
                        })
                        .collect();
                    wb.show_debug_pick(items, "Select the container to debug in", window, cx);
                }
            });
        })
        .detach();
    }

    /// The .NET processes in `container` (attaches right away when there is one).
    pub(super) fn open_container_process_picker(&mut self, container: String, window: &mut Window, cx: &mut Context<Self>) {
        let name = container.clone();
        let task = cx.background_spawn(async move { bf_core::container::app_candidates(&bf_core::container::processes(&name)) });
        cx.spawn_in(window, async move |this, cx| {
            let apps = task.await;
            let _ = this.update_in(cx, |wb, window, cx| match apps.as_slice() {
                [] => {
                    wb.log(format!("No app process found in {container}."));
                    wb.show_panel(PanelView::Output, cx);
                }
                [only] => wb.debug_attach_container(container.clone(), only.pid, window, cx),
                many => {
                    let items = many
                        .iter()
                        .map(|p| {
                            let mut item = PaletteItem::new(format!("{}  ({})", short(&p.command, 80), p.pid), PaletteAction::Command(Box::new(AttachToContainerPid { container: container.clone(), pid: p.pid })));
                            item.search = Some(format!("{} {}", p.pid, p.command));
                            item
                        })
                        .collect();
                    wb.show_debug_pick(items, &format!("Select the process in {container} to attach to"), window, cx);
                }
            });
        })
        .detach();
    }

    fn show_debug_pick(&mut self, items: Vec<PaletteItem>, placeholder: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_open = None;
        let palette = cx.new(|cx| Palette::new(Mode::Pick, items, placeholder, "", window, cx));
        let sub = cx.subscribe_in(&palette, window, Self::on_palette_event);
        palette.update(cx, |p, cx| p.focus_input(window, cx));
        self.palette = Some((palette, sub));
        cx.notify();
    }

    /// Attaches to `pid` in `container`: puts netcoredbg into the container first when it isn't there (the
    /// host's own when it can run there, else the Linux release for the container's architecture).
    pub(super) fn debug_attach_container(&mut self, container: String, pid: u32, window: &mut Window, cx: &mut Context<Self>) {
        if self.debug.is_some() {
            self.log("Stop the current debug session first (Shift+F5).");
            self.show_panel(PanelView::Output, cx);
            return;
        }
        let Some(root) = self.workspace.root() else { return };
        self.debug_generation += 1;
        let generation = self.debug_generation;
        let mut session = DebugSession::new(generation, format!("{container} (process {pid})"), Phase::Starting);
        session.container = Some(ContainerPaths { root, files: self.workspace.files() });
        self.debug = Some(session);
        self.debug_console.update(cx, |c, cx| c.reload_text("", cx));
        self.debug_print(format!("Attaching to process {pid} in container {container}…\n"), cx);
        self.show_side(SideView::Debug, cx);
        self.show_panel(PanelView::DebugConsole, cx);

        let host = bf_dap::netcoredbg_command(self.settings.get_string("debug.netcoredbgPath").as_deref());
        let cache = self.paths.cache_dir();
        let (tx, progress) = flume::unbounded::<String>();
        let name = container.clone();
        let prepare = cx.background_spawn(async move {
            use bf_core::container as c;
            if c::debugger_ready(&name) {
                return Ok(());
            }
            let arch = c::architecture(&name).unwrap_or_else(|| "amd64".into());
            let dir = match host.as_deref().and_then(|h| c::host_debugger_dir(h, &arch)) {
                Some(dir) => dir,
                None => {
                    let _ = tx.send(format!("Downloading netcoredbg for linux-{arch}…\n"));
                    c::download_debugger(&arch, &cache)?
                }
            };
            let _ = tx.send(format!("Copying netcoredbg into {name} ({})…\n", bf_core::container::DEBUGGER_DIR));
            c::install_debugger(&name, &dir)
        });
        let progress_task = cx.spawn(async move |this, cx| {
            while let Ok(line) = progress.recv_async().await {
                let _ = this.update(cx, |wb, cx| wb.debug_print(line, cx));
            }
        });
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = prepare.await;
            let _ = this.update_in(cx, |wb, window, cx| {
                if !wb.same_session(generation) {
                    return;
                }
                match result {
                    Ok(()) => wb.debug_connect(generation, Target::Container { container, pid }, String::new(), window, cx),
                    Err(e) => {
                        wb.debug_print(format!("Could not debug in the container: {e:#}\n"), cx);
                        wb.debug_end(cx);
                    }
                }
            });
        });
        if let Some(session) = self.debug.as_mut() {
            session._tasks.extend([progress_task, task]);
        }
    }

    fn debug_connect(&mut self, generation: u64, target: Target, adapter: String, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = match &target {
            Target::Launch { project, .. } => project.parent().map(Path::to_path_buf),
            Target::Attach { .. } | Target::Container { .. } => self.workspace.root(),
        }
        .unwrap_or_else(|| PathBuf::from("."));
        let (command, args) = match &target {
            Target::Container { container, .. } => ("docker".to_owned(), bf_core::container::adapter_args(container)),
            _ => (adapter, vec!["--interpreter=vscode".into()]),
        };
        let (client, events) = match DapClient::start(AdapterConfig { command, args, cwd: cwd.clone() }) {
            Ok(started) => started,
            Err(e) => {
                self.debug_print(format!("{e:#}\n"), cx);
                self.debug_end(cx);
                return;
            }
        };
        let just_my_code: bool = self.settings.get("debug.justMyCode", true);
        let mut profile = None;
        let (arguments, attached) = match &target {
            Target::Launch { project, program } => {
                profile = launch_profile(project, self.chosen_launch_profile(project).as_deref());
                let mut env = serde_json::Map::new();
                if let Some(p) = &profile {
                    for (key, value) in &p.env {
                        env.insert(key.clone(), json!(value));
                    }
                    if let Some(urls) = &p.application_url {
                        env.entry("ASPNETCORE_URLS").or_insert(json!(urls));
                    }
                }
                if is_web_project(project) {
                    env.entry("ASPNETCORE_ENVIRONMENT").or_insert(json!("Development"));
                }
                let args = profile.as_ref().map(|p| p.args.clone()).unwrap_or_default();
                let via = profile.as_ref().map(|p| format!(" (launch profile \"{}\")", p.name)).unwrap_or_default();
                self.debug_print(format!("Launching {}{via}\n", program.display()), cx);
                let arguments = json!({
                    "name": ".NET Launch",
                    "type": "coreclr",
                    "request": "launch",
                    "program": program,
                    "args": args,
                    "cwd": cwd,
                    "env": env,
                    "stopAtEntry": self.settings.get("debug.stopAtEntry", false),
                    "justMyCode": just_my_code,
                    "enableStepFiltering": true,
                });
                (arguments, false)
            }
            Target::Attach { pid } | Target::Container { pid, .. } => (
                json!({ "name": ".NET Attach", "type": "coreclr", "request": "attach", "processId": pid, "justMyCode": just_my_code }),
                true,
            ),
        };

        // initialize → launch/attach. The adapter answers launch only after configurationDone, which is
        // sent when the `initialized` event arrives (see `on_debug_event`).
        let init = client.initialize();
        let requester = client.clone();
        let start_task = cx.spawn(async move |this, cx| {
            let result = match init.recv_async().await {
                Ok(Ok(_)) => {
                    let rx = if attached { requester.attach(arguments) } else { requester.launch(arguments) };
                    rx.recv_async().await.unwrap_or_else(|_| Err(anyhow!("no response from the debugger")))
                }
                Ok(Err(e)) => Err(e),
                Err(_) => Err(anyhow!("no response from the debugger")),
            };
            if let Err(e) = result {
                let _ = this.update(cx, |wb, cx| {
                    if wb.same_session(generation) {
                        wb.debug_print(format!("Could not {}: {e:#}\n", if attached { "attach" } else { "launch" }), cx);
                        wb.debug_end(cx);
                    }
                });
            }
        });
        let event_task = cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = events.recv_async().await {
                if this.update_in(cx, |wb, window, cx| wb.on_debug_event(generation, event, window, cx)).is_err() {
                    break;
                }
            }
        });
        if let Some(session) = self.debug.as_mut() {
            session.client = Some(client);
            session.attached = attached;
            session.profile = profile;
            session.phase = Phase::Starting;
            session._tasks.extend([start_task, event_task]);
        }
        cx.notify();
    }

    fn on_debug_event(&mut self, generation: u64, event: DapEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.same_session(generation) {
            return;
        }
        match event {
            DapEvent::Initialized => {
                let Some(session) = self.debug.as_mut() else { return };
                session.configured = true;
                if session.phase == Phase::Starting {
                    session.phase = Phase::Running;
                }
                let client = session.client.clone();
                let paths: Vec<PathBuf> = self.breakpoints.keys().cloned().collect();
                for path in paths {
                    self.debug_send_breakpoints(&path, cx);
                }
                if let Some(client) = client {
                    let filters: Vec<String> = self.settings.get("debug.breakOnExceptions", vec!["user-unhandled".to_owned()]);
                    let filters: Vec<&str> = filters.iter().map(String::as_str).collect();
                    let _ = client.set_exception_breakpoints(&filters);
                    let _ = client.configuration_done();
                }
            }
            DapEvent::Stopped { reason, thread_id, description, text, .. } => {
                let Some(session) = self.debug.as_mut() else { return };
                session.phase = Phase::Stopped;
                session.thread_id = thread_id.or(session.thread_id);
                session.stop_reason = description.unwrap_or_else(|| reason.clone());
                session.stop_kind = reason.clone();
                session.clear_stop();
                if reason == "exception" {
                    self.debug_print(format!("Exception: {}\n", text.unwrap_or_default()), cx);
                }
                // The Debug view and the window come up once the stack shows it's a real stop (not a
                // logpoint or a hit count still to reach).
                self.debug_fetch_stack(window, cx);
            }
            DapEvent::Continued { .. } => {
                if let Some(session) = self.debug.as_mut()
                    && session.phase == Phase::Stopped
                {
                    session.phase = Phase::Running;
                    session.clear_stop();
                    self.debug_clear_exec(cx);
                }
            }
            DapEvent::Output { category, text } => {
                if category == "telemetry" {
                    return;
                }
                self.maybe_open_browser(&text);
                self.debug_print(text, cx);
            }
            DapEvent::Breakpoint { breakpoint, .. } => {
                let Some(session) = self.debug.as_ref() else { return };
                let Some((path, line)) = breakpoint.id.and_then(|id| session.bp_ids.get(&id)).cloned() else { return };
                if let Some(b) = self.breakpoints.get_mut(&path).and_then(|list| list.iter_mut().find(|b| b.line == line)) {
                    b.verified = breakpoint.verified;
                }
                self.sync_editor_breakpoints(&path, cx);
            }
            DapEvent::Thread { .. } | DapEvent::Module(_) => {}
            DapEvent::Exited(code) => self.debug_print(format!("The program exited with code {code}.\n"), cx),
            DapEvent::Terminated => {
                self.debug_print("Debugging ended.\n", cx);
                self.debug_end(cx);
            }
            DapEvent::AdapterExited(reason) => {
                self.debug_print(format!("{reason}\n"), cx);
                self.debug_end(cx);
            }
        }
        cx.notify();
    }

    /// Opens the browser once an ASP.NET Core app reports its URL, if its launch profile asks for it.
    fn maybe_open_browser(&mut self, text: &str) {
        let enabled: bool = self.settings.get("debug.openBrowser", true);
        let Some(session) = self.debug.as_mut() else { return };
        if !enabled || session.browser_opened {
            return;
        }
        let Some(profile) = &session.profile else { return };
        if let Some(url) = text.lines().find_map(listening_url).and_then(|url| profile.browser_url(url)) {
            session.browser_opened = true;
            open_browser(&url);
        }
    }

    /// Ends the session's UI state. Dropping the last client handle stops the adapter.
    fn debug_end(&mut self, cx: &mut Context<Self>) {
        self.debug_clear_exec(cx);
        self.debug = None;
        // The container started for this session goes with it.
        if let Some(plan) = self.debug_container.take() {
            self.debug_print(format!("Stopping the container {}…\n", plan.name), cx);
            cx.background_spawn(async move { plan.remove() }).detach();
        }
        for list in self.breakpoints.values_mut() {
            for b in list.iter_mut() {
                b.verified = true;
            }
        }
        let paths: Vec<PathBuf> = self.breakpoints.keys().cloned().collect();
        for path in paths {
            self.sync_editor_breakpoints(&path, cx);
        }
        cx.notify();
    }

    /// Shift+F5: ends the session (a launched program is terminated, an attached one detached).
    pub(super) fn debug_stop(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &self.debug else { return };
        if session.phase == Phase::Building {
            if let Some(d) = &self.dotnet {
                d.kill();
            }
            self.log("Debugging cancelled.");
            self.debug = None;
            cx.notify();
            return;
        }
        if let Some(client) = session.client.clone() {
            let rx = client.disconnect(!session.attached);
            // Give the adapter a moment to end or detach from the program, then make sure it's gone.
            std::thread::spawn(move || {
                let _ = rx.recv_timeout(Duration::from_secs(3));
                client.kill();
            });
        }
        self.debug_print("Debugging stopped.\n", cx);
        self.debug_end(cx);
    }

    pub(super) fn debug_restart(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.debug.as_ref().is_some_and(|s| s.attached) {
            self.debug_print("Restart is not available when attached to a process.\n", cx);
            return;
        }
        if self.debug.is_none() {
            self.debug_start(window, cx);
            return;
        }
        self.debug_stop(cx);
        // Let the old program release its ports (ASP.NET Core) before starting again.
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(800)).await;
            let _ = this.update_in(cx, |wb, window, cx| wb.debug_start(window, cx));
        })
        .detach();
    }

    // ───────────── execution control ─────────────

    pub(super) fn debug_continue(&mut self, cx: &mut Context<Self>) {
        self.debug_resume(|client, thread| client.continue_(thread), cx);
    }

    pub(super) fn debug_step(&mut self, step: Step, cx: &mut Context<Self>) {
        self.debug_resume(
            move |client, thread| match step {
                Step::Over => client.next(thread),
                Step::Into => client.step_in(thread),
                Step::Out => client.step_out(thread),
            },
            cx,
        );
    }

    fn debug_resume(&mut self, send: impl FnOnce(&DapClient, i64) -> flume::Receiver<anyhow::Result<serde_json::Value>>, cx: &mut Context<Self>) {
        let Some(session) = self.debug.as_mut().filter(|s| s.phase == Phase::Stopped) else { return };
        let (Some(client), Some(thread)) = (session.client.clone(), session.thread_id) else { return };
        session.phase = Phase::Running;
        session.clear_stop();
        let _ = send(&client, thread);
        self.debug_clear_exec(cx);
        cx.notify();
    }

    pub(super) fn debug_pause(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.debug.as_ref().filter(|s| s.phase == Phase::Running) else { return };
        let Some(client) = session.client.clone() else { return };
        // netcoredbg stops every thread; it only needs one id.
        let threads = client.threads();
        cx.background_spawn(async move {
            if let Ok(Ok(threads)) = threads.recv_async().await
                && let Some(thread) = threads.first()
            {
                let _ = client.pause(thread.id);
            }
        })
        .detach();
    }

    fn debug_clear_exec(&mut self, cx: &mut Context<Self>) {
        for tab in &self.tabs {
            if tab.editor.read(cx).exec_line.is_some() {
                tab.editor.update(cx, |e, cx| e.set_exec_line(None, cx));
            }
        }
        self.set_hover_evaluator(None, cx);
    }

    /// While paused: hovering an expression in an editor shows its value in frame `frame` (`None`: running).
    fn set_hover_evaluator(&self, target: Option<(DapClient, i64)>, cx: &mut Context<Self>) {
        let evaluator: Option<crate::editor::DebugEvaluator> =
            target.map(|(client, frame)| std::sync::Arc::new(move |expr: &str| client.evaluate(expr, Some(frame), EvaluateContext::Hover)) as crate::editor::DebugEvaluator);
        for tab in &self.tabs {
            tab.editor.update(cx, |e, _| e.debug_evaluator = evaluator.clone());
        }
    }

    // ───────────── watch expressions ─────────────

    fn watches_key(&self) -> Option<String> {
        self.workspace.root().map(|root| format!("watches:{}", root.display()))
    }

    pub(super) fn load_watches(&mut self) {
        self.watches = self.watches_key().and_then(|k| self.state.get(&k)).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    }

    fn save_watches(&self) {
        let Some(key) = self.watches_key() else { return };
        let text = serde_json::to_string(&self.watches).unwrap_or_default();
        self.state.set(&key, (!self.watches.is_empty()).then_some(text.as_str()));
    }

    pub(super) fn add_watch(&mut self, expr: String, cx: &mut Context<Self>) {
        if expr.is_empty() || self.watches.contains(&expr) {
            return;
        }
        self.watches.push(expr);
        self.save_watches();
        self.refresh_watches(cx);
    }

    fn remove_watch(&mut self, expr: &str, cx: &mut Context<Self>) {
        self.watches.retain(|w| w != expr);
        self.save_watches();
        cx.notify();
    }

    /// Evaluates every watch in the selected frame (after each stop and frame change).
    fn refresh_watches(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.debug.as_ref().filter(|s| s.phase == Phase::Stopped) else {
            cx.notify();
            return;
        };
        let (Some(client), Some(frame)) = (session.client.clone(), session.frames.get(session.frame).map(|f| f.id)) else { return };
        let (generation, index) = (session.generation, session.frame);
        for expr in self.watches.clone() {
            let rx = client.evaluate(&expr, Some(frame), EvaluateContext::Watch);
            cx.spawn(async move |this, cx| {
                let value = match rx.recv_async().await {
                    Ok(Ok(r)) => r.result,
                    Ok(Err(e)) => format!("<{}>", short(&format!("{e:#}"), 80)),
                    Err(_) => "<no answer>".into(),
                };
                let _ = this.update(cx, |wb, cx| {
                    if let Some(s) = wb.debug.as_mut().filter(|s| s.generation == generation && s.frame == index) {
                        s.watch_values.insert(expr, value);
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }

    fn debug_fetch_stack(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = &self.debug else { return };
        let (Some(client), Some(thread)) = (session.client.clone(), session.thread_id) else { return };
        let generation = session.generation;
        let rx = client.stack_trace(thread, 200);
        cx.spawn_in(window, async move |this, cx| {
            let Ok(result) = rx.recv_async().await else { return };
            let _ = this.update_in(cx, |wb, window, cx| {
                let Some(session) = wb.debug.as_mut().filter(|s| s.generation == generation) else { return };
                match result {
                    Ok(mut frames) => {
                        if let Some(c) = &session.container {
                            for frame in &mut frames {
                                if let Some(path) = &frame.path
                                    && !path.is_file()
                                {
                                    frame.path = bf_core::container::host_path(&path.to_string_lossy(), &c.files).or(frame.path.take());
                                }
                            }
                        }
                        // Start at the first frame with source on disk (user code).
                        let first = frames.iter().position(|f| f.path.as_ref().is_some_and(|p| p.is_file())).unwrap_or(0);
                        let top = frames.first().cloned();
                        session.frames = frames;
                        if wb.breakpoint_passes(top.as_ref(), cx) {
                            return;
                        }
                        wb.show_side(SideView::Debug, cx);
                        window.activate_window();
                        wb.debug_select_frame(first, window, cx);
                    }
                    Err(e) => wb.debug_print(format!("Could not read the call stack: {e:#}\n"), cx),
                }
            });
        })
        .detach();
    }

    /// After a stop at a breakpoint with a hit count or a log message: counts the hit, prints the
    /// logpoint's message and continues when the program shouldn't stay stopped. `true` when it continued.
    fn breakpoint_passes(&mut self, top: Option<&StackFrame>, cx: &mut Context<Self>) -> bool {
        let Some(session) = self.debug.as_mut().filter(|s| s.stop_kind == "breakpoint") else { return false };
        let Some(frame) = top else { return false };
        let Some(path) = frame.path.as_ref().map(|p| normalize_path(p)) else { return false };
        let line = frame.line.saturating_sub(1) as usize;
        let Some(bp) = self.breakpoints.iter().find(|(p, _)| bf_core::paths::same_path(p, &path)).and_then(|(_, list)| list.iter().find(|b| b.line == line)).cloned() else {
            return false;
        };
        if let Some(condition) = &bp.hit_condition {
            let hits = session.hits.entry((path.clone(), line)).or_insert(0);
            *hits += 1;
            if !bf_core::debug::hit_condition_met(condition, *hits) {
                self.debug_continue(cx);
                return true;
            }
        }
        let Some(message) = bp.log_message.clone() else { return false };
        let (Some(client), frame_id) = (session.client.clone(), frame.id) else { return false };
        let generation = session.generation;
        let parts = bf_core::debug::log_message_parts(&message);
        cx.spawn(async move |this, cx| {
            let mut text = String::new();
            for part in parts {
                match part {
                    bf_core::debug::LogPart::Text(t) => text.push_str(&t),
                    bf_core::debug::LogPart::Expr(e) => {
                        let value = match client.evaluate(&e, Some(frame_id), EvaluateContext::Watch).recv_async().await {
                            Ok(Ok(r)) => r.result,
                            Ok(Err(err)) => format!("<{e}: {err}>"),
                            Err(_) => format!("<{e}>"),
                        };
                        text.push_str(&value);
                    }
                }
            }
            let _ = this.update(cx, |wb, cx| {
                if wb.same_session(generation) {
                    wb.debug_print(format!("{text}\n"), cx);
                    wb.debug_continue(cx);
                }
            });
        })
        .detach();
        true
    }

    /// Debug: Add Conditional Breakpoint / Add Logpoint / Edit Hit Count on the caret's line: asks for the
    /// value (empty removes it) with the current one filled in.
    pub(super) fn edit_breakpoint_option(&mut self, option: BreakpointOption, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor() else { return };
        let (path, line, glyphs) = {
            let e = editor.read(cx);
            (e.path.clone(), e.state.caret().line, e.glyph_margin)
        };
        let Some(path) = path.filter(|_| glyphs) else {
            self.log("Breakpoints work in C# and Razor files.");
            return;
        };
        let current = self.breakpoints.get(&path).and_then(|l| l.iter().find(|b| b.line == line)).and_then(|b| match option {
            BreakpointOption::Condition => b.condition.clone(),
            BreakpointOption::HitCount => b.hit_condition.clone(),
            BreakpointOption::LogMessage => b.log_message.clone(),
        });
        self.open_prompt(PromptKind::BreakpointOption { path, line, option }, &current.unwrap_or_default(), window, cx);
    }

    /// Sets one option of the breakpoint on `line` (adding the breakpoint when there is none); an empty
    /// value clears the option.
    pub(super) fn set_breakpoint_option(&mut self, path: PathBuf, line: usize, option: BreakpointOption, value: &str, cx: &mut Context<Self>) {
        let value = Some(value.trim().to_owned()).filter(|v| !v.is_empty());
        let list = self.breakpoints.entry(path.clone()).or_default();
        let at = match list.iter().position(|b| b.line == line) {
            Some(i) => i,
            None => {
                list.push(Breakpoint::new(line));
                list.sort_by_key(|b| b.line);
                list.iter().position(|b| b.line == line).unwrap_or(0)
            }
        };
        let bp = &mut list[at];
        match option {
            BreakpointOption::Condition => bp.condition = value,
            BreakpointOption::HitCount => bp.hit_condition = value,
            BreakpointOption::LogMessage => bp.log_message = value,
        }
        if let Some(session) = self.debug.as_mut() {
            session.hits.remove(&(path.clone(), line));
        }
        self.sync_editor_breakpoints(&path, cx);
        self.save_breakpoints();
        self.debug_send_breakpoints(&path, cx);
        cx.notify();
    }

    fn debug_select_frame(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.debug.as_mut() else { return };
        let Some(frame) = session.frames.get(ix).cloned() else { return };
        session.frame = ix;
        session.scopes.clear();
        session.children.clear();
        session.expanded.clear();
        let (client, generation) = (session.client.clone(), session.generation);
        self.debug_clear_exec(cx);
        if let Some(path) = frame.path.as_ref().filter(|p| p.is_file()) {
            let path = normalize_path(path);
            let at = Point::new(frame.line.saturating_sub(1) as usize, frame.column.saturating_sub(1) as usize);
            self.open_file(path.clone(), Some(at), window, cx);
            if let Some(tab) = self.tabs.iter().find(|t| t.path.as_deref().is_some_and(|p| bf_core::paths::same_path(p, &path))) {
                tab.editor.update(cx, |e, cx| e.set_exec_line(Some((at.line, ix == 0)), cx));
            }
        }
        let Some(client) = client else { return };
        self.set_hover_evaluator(Some((client.clone(), frame.id)), cx);
        if let Some(s) = self.debug.as_mut() {
            s.watch_values.clear();
        }
        self.refresh_watches(cx);
        let rx = client.scopes(frame.id);
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.recv_async().await else { return };
            let _ = this.update(cx, |wb, cx| {
                let Some(session) = wb.debug.as_mut().filter(|s| s.generation == generation && s.frame == ix) else { return };
                session.scopes = result.unwrap_or_default();
                // Expand the first cheap scope (usually Locals).
                let first = session.scopes.iter().find(|s| !s.expensive).map(|s| s.variables_reference);
                if let Some(reference) = first {
                    wb.debug_toggle_variable(reference, cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn debug_toggle_variable(&mut self, reference: i64, cx: &mut Context<Self>) {
        let Some(session) = self.debug.as_mut() else { return };
        if session.expanded.remove(&reference) {
            cx.notify();
            return;
        }
        session.expanded.insert(reference);
        if !session.children.contains_key(&reference)
            && let Some(client) = session.client.clone()
        {
            let generation = session.generation;
            let rx = client.variables(reference);
            cx.spawn(async move |this, cx| {
                let Ok(result) = rx.recv_async().await else { return };
                let _ = this.update(cx, |wb, cx| {
                    if let Some(session) = wb.debug.as_mut().filter(|s| s.generation == generation) {
                        let children = result.unwrap_or_else(|e| {
                            vec![Variable { name: "error".into(), value: format!("{e:#}"), type_name: None, variables_reference: 0 }]
                        });
                        session.children.insert(reference, children);
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }

    /// Evaluates a Debug Console expression in the selected frame.
    pub(super) fn debug_evaluate(&mut self, expression: String, cx: &mut Context<Self>) {
        self.debug_print(format!("> {expression}\n"), cx);
        let Some(session) = &self.debug else {
            self.debug_print("No debug session is active.\n", cx);
            return;
        };
        let Some(client) = session.client.clone() else { return };
        let frame_id = if session.phase == Phase::Stopped { session.frames.get(session.frame).map(|f| f.id) } else { None };
        let rx = client.evaluate(&expression, frame_id, EvaluateContext::Repl);
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.recv_async().await else { return };
            let _ = this.update(cx, |wb, cx| {
                let text = match result {
                    Ok(r) => match r.type_name {
                        Some(t) => format!("{}  ({t})\n", r.result),
                        None => format!("{}\n", r.result),
                    },
                    Err(e) => format!("{e:#}\n"),
                };
                wb.debug_print(text, cx);
            });
        })
        .detach();
    }

    // ───────────── views ─────────────

    /// The Run and Debug side view: toolbar, call stack, variables and breakpoints.
    pub(super) fn render_debug_view(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = Theme::global(cx);
        let muted = t.c("foreground.muted");
        let row_h = t.n("list.rowHeight", 22.0);
        let icon_fg = t.c("icon.foreground");
        let hover = t.c("list.hoverBackground");
        let section = |title: &'static str| {
            div()
                .w_full()
                .h(px(row_h))
                .mt_2()
                .px_3()
                .flex()
                .items_center()
                .text_size(t.m("sideBarSectionHeader.fontSize", 11.0))
                .font_weight(t.weight("sideBarSectionHeader.weight", gpui::FontWeight::BOLD))
                .text_color(t.c("sideBarSectionHeader.foreground"))
                .child(title)
        };
        let tool = |id: &'static str, icon_name: &'static str, tip: SharedString, color: gpui::Hsla| {
            div()
                .id(id)
                .p(px(4.))
                .rounded(px(4.))
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .tooltip(tooltip(tip))
                .child(icon(icon_name, px(16.), color))
        };
        let button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .mx_3()
                .px_3()
                .py_1()
                .rounded(t.radius("button.radius", 4.0))
                .bg(t.bg("button.background"))
                .text_color(t.c("button.foreground"))
                .cursor_pointer()
                .flex()
                .justify_center()
                .hover(|s| s.bg(t.c("button.hoverBackground")))
                .child(label)
        };

        let mut col = div().id("debug-view").size_full().flex().flex_col().overflow_y_scroll().pb_3();
        match &self.debug {
            None => {
                col = col
                    .child(div().h_3())
                    .child(button("debug-start", "Start Debugging (F5)").on_click(cx.listener(|wb, _, w, cx| wb.debug_start(w, cx))))
                    .child(div().h_2())
                    .child(button("debug-attach", "Attach to .NET Process…").on_click(cx.listener(|wb, _, w, cx| wb.open_attach_picker(w, cx))))
                    .child(div().px_3().pt_3().text_size(px(12.)).text_color(muted).child(
                        "Builds the startup project (\"dotnet.project\" in settings, else the first runnable .csproj) and runs it under \
                         netcoredbg. ASP.NET Core apps use their launch profile. Click left of the line numbers or press F9 to set a \
                         breakpoint; this works in .cs, .razor and .cshtml files.",
                    ));
            }
            Some(session) => {
                let stopped = session.phase == Phase::Stopped;
                let running = session.phase == Phase::Running;
                let on = icon_fg;
                let off = muted;
                let toolbar = div()
                    .px_2()
                    .pt_2()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(if stopped || !running {
                        tool("dbg-continue", "play", "Continue (F5)".into(), if stopped { t.c_or("debugIcon.continueForeground", 0x75beffff) } else { off })
                            .on_click(cx.listener(|wb, _, _, cx| wb.debug_continue(cx)))
                    } else {
                        tool("dbg-pause", "pause", "Pause (F6)".into(), t.c_or("debugIcon.pauseForeground", 0x75beffff)).on_click(cx.listener(|wb, _, _, cx| wb.debug_pause(cx)))
                    })
                    .child(tool("dbg-over", "stepOver", "Step Over (F10)".into(), if stopped { on } else { off }).on_click(cx.listener(|wb, _, _, cx| wb.debug_step(Step::Over, cx))))
                    .child(tool("dbg-into", "stepInto", "Step Into (F11)".into(), if stopped { on } else { off }).on_click(cx.listener(|wb, _, _, cx| wb.debug_step(Step::Into, cx))))
                    .child(tool("dbg-out", "stepOut", "Step Out (Shift+F11)".into(), if stopped { on } else { off }).on_click(cx.listener(|wb, _, _, cx| wb.debug_step(Step::Out, cx))))
                    .child(tool("dbg-restart", "refresh", format!("Restart ({})", crate::actions::key_label("ctrl-shift-f5")).into(), t.c_or("debugIcon.restartForeground", 0x89d185ff)).on_click(cx.listener(|wb, _, w, cx| wb.debug_restart(w, cx))))
                    .child(tool("dbg-stop", "stop", "Stop (Shift+F5)".into(), t.c_or("debugIcon.stopForeground", 0xf48771ff)).on_click(cx.listener(|wb, _, _, cx| wb.debug_stop(cx))));
                let status = if stopped && !session.stop_reason.is_empty() {
                    format!("Paused on {}", session.stop_reason)
                } else {
                    session.status_text()
                };
                col = col.child(toolbar).child(div().px_3().pt_1().text_size(px(12.)).text_color(muted).child(status));

                if stopped {
                    col = col.child(section("CALL STACK"));
                    for (i, frame) in session.frames.iter().enumerate().take(200) {
                        let file = frame.path.as_ref().and_then(|p| p.file_name()).map(|n| format!("{}:{}", n.to_string_lossy(), frame.line));
                        let has_source = frame.path.as_ref().is_some_and(|p| p.is_file());
                        let selected = i == session.frame;
                        col = col.child(
                            div()
                                .id(SharedString::from(format!("frame-{i}")))
                                .h(px(row_h))
                                .px_3()
                                .flex()
                                .items_center()
                                .gap_2()
                                .cursor_pointer()
                                .text_size(t.m("list.fontSize", 13.0))
                                .when(selected, |d| d.bg(t.c("list.activeSelectionBackground")))
                                .hover(|s| s.bg(hover))
                                .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().when(!has_source, |d| d.text_color(muted)).child(frame.name.clone()))
                                .children(file.map(|f| div().flex_none().text_size(px(11.)).text_color(muted).child(f)))
                                .on_click(cx.listener(move |wb, _, w, cx| wb.debug_select_frame(i, w, cx))),
                        );
                    }

                    col = col.child(section("VARIABLES"));
                    for scope in &session.scopes {
                        let reference = scope.variables_reference;
                        let open = session.expanded.contains(&reference);
                        col = col.child(
                            div()
                                .id(SharedString::from(format!("scope-{reference}")))
                                .h(px(row_h))
                                .px_2()
                                .flex()
                                .items_center()
                                .gap_1()
                                .cursor_pointer()
                                .hover(|s| s.bg(hover))
                                .child(icon(if open { "chevronDown" } else { "chevronRight" }, px(14.), icon_fg))
                                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(scope.name.clone()))
                                .on_click(cx.listener(move |wb, _, _, cx| wb.debug_toggle_variable(reference, cx))),
                        );
                        if open {
                            let mut rows = Vec::new();
                            self.debug_variable_rows(session, reference, 1, &mut rows, cx);
                            col = col.children(rows);
                        }
                    }
                }
            }
        }

        // Watch expressions, evaluated in the selected frame while paused.
        col = col.child(
            div()
                .flex()
                .items_center()
                .child(div().flex_1().child(section("WATCH")))
                .child(div().mt_2().mr_2().child(tool("watch-add", "add", "Add Watch Expression".into(), icon_fg).on_click(cx.listener(|wb, _, w, cx| wb.open_prompt(PromptKind::WatchExpression, "", w, cx))))),
        );
        if self.watches.is_empty() {
            col = col.child(div().px_3().text_size(px(12.)).text_color(muted).child("Add expressions to see their values while paused."));
        }
        let paused = self.debug.as_ref().is_some_and(|s| s.phase == Phase::Stopped);
        for (i, expr) in self.watches.iter().enumerate() {
            let value = self.debug.as_ref().and_then(|s| s.watch_values.get(expr).cloned());
            let shown = match (paused, value) {
                (true, Some(v)) => v,
                (true, None) => "…".into(),
                (false, _) => "not paused".into(),
            };
            let remove = expr.clone();
            col = col.child(
                div()
                    .id(SharedString::from(format!("watch-{i}")))
                    .group("watch-row")
                    .h(px(row_h))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(t.m("list.fontSize", 13.0))
                    .hover(|s| s.bg(hover))
                    .child(div().flex_none().whitespace_nowrap().font_family(t.editor_font.clone()).child(expr.clone()))
                    .child(div().flex_none().text_color(muted).child("="))
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().font_family(t.editor_font.clone()).text_color(if paused { t.c("editor.foreground") } else { muted }).child(shown))
                    .child(
                        div()
                            .id(SharedString::from(format!("watch-x-{i}")))
                            .invisible()
                            .group_hover("watch-row", |s| s.visible())
                            .cursor_pointer()
                            .child(icon("close", px(14.), icon_fg))
                            .on_click(cx.listener(move |wb, _, _, cx| wb.remove_watch(&remove, cx))),
                    ),
            );
        }

        // Breakpoints of every file.
        col = col.child(section("BREAKPOINTS"));
        if self.breakpoints.is_empty() {
            col = col.child(div().px_3().text_size(px(12.)).text_color(muted).child("None. Press F9 on a line, or click left of its number."));
        }
        let root = self.workspace.root();
        for (path, list) in &self.breakpoints {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let dir = path.parent().map(|p| root.as_ref().and_then(|r| p.strip_prefix(r).ok()).unwrap_or(p).display().to_string()).unwrap_or_default();
            for b in list {
                let (p1, p2, line) = (path.clone(), path.clone(), b.line);
                let color = if b.verified { t.c_or("debugIcon.breakpointForeground", 0xe51400ff) } else { t.c_or("debugIcon.breakpointUnverifiedForeground", 0x848484ff) };
                col = col.child(
                    div()
                        .id(SharedString::from(format!("bp-{}-{line}", path.display())))
                        .group("bp-row")
                        .h(px(row_h))
                        .px_3()
                        .flex()
                        .items_center()
                        .gap_2()
                        .cursor_pointer()
                        .text_size(t.m("list.fontSize", 13.0))
                        .hover(|s| s.bg(hover))
                        .child(icon("breakpoint", px(14.), color))
                        .child(div().whitespace_nowrap().child(name.clone()))
                        .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_size(px(11.)).text_color(muted).child(breakpoint_detail(b).unwrap_or_else(|| dir.clone())))
                        .child(div().flex_none().text_size(px(11.)).text_color(muted).child((line + 1).to_string()))
                        .child(
                            div()
                                .id(SharedString::from(format!("bp-x-{}-{line}", path.display())))
                                .invisible()
                                .group_hover("bp-row", |s| s.visible())
                                .child(icon("close", px(14.), icon_fg))
                                .on_click(cx.listener(move |wb, _, _, cx| {
                                    cx.stop_propagation();
                                    wb.remove_breakpoint(&p1, line, cx);
                                })),
                        )
                        .on_click(cx.listener(move |wb, _, w, cx| wb.open_file(p2.clone(), Some(Point::new(line, 0)), w, cx))),
                );
            }
        }
        if !self.breakpoints.is_empty() {
            col = col.child(div().h_2()).child(button("bp-remove-all", "Remove All Breakpoints").on_click(cx.listener(|wb, _, _, cx| wb.remove_all_breakpoints(cx))));
        }
        col.into_any_element()
    }

    /// Rows for the children of `reference`, recursing into expanded ones.
    fn debug_variable_rows(&self, session: &DebugSession, reference: i64, depth: usize, rows: &mut Vec<gpui::AnyElement>, cx: &mut Context<Self>) {
        let t = Theme::global(cx);
        let Some(children) = session.children.get(&reference) else {
            rows.push(div().pl(px(8. + depth as f32 * 14.)).text_size(px(12.)).text_color(t.c("foreground.muted")).child("Loading…").into_any_element());
            return;
        };
        let row_h = t.n("list.rowHeight", 22.0);
        let name_color = t.c_or("debugTokenExpression.name", 0xc586c0ff);
        let value_color = t.c_or("debugTokenExpression.value", 0xccccccff);
        let muted = t.c("foreground.muted");
        let hover = t.c("list.hoverBackground");
        let icon_fg = t.c("icon.foreground");
        for (i, var) in children.iter().enumerate().take(500) {
            let child = var.variables_reference;
            let open = child > 0 && session.expanded.contains(&child);
            let mut row = div()
                .id(SharedString::from(format!("var-{reference}-{i}")))
                .h(px(row_h))
                .pl(px(8. + depth as f32 * 14.))
                .pr_2()
                .flex()
                .items_center()
                .gap_1()
                .text_size(t.m("list.fontSize", 13.0))
                .hover(|s| s.bg(hover))
                .child(div().w(px(14.)).flex_none().when(child > 0, |d| d.child(icon(if open { "chevronDown" } else { "chevronRight" }, px(14.), icon_fg))))
                .child(div().flex_none().whitespace_nowrap().text_color(name_color).child(format!("{}:", var.name)))
                .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_color(value_color).child(var.value.clone()))
                .when_some(var.type_name.clone(), |d, ty| d.tooltip(tooltip(ty)))
                .text_color(muted);
            if child > 0 {
                row = row.cursor_pointer().on_click(cx.listener(move |wb, _, _, cx| wb.debug_toggle_variable(child, cx)));
            }
            rows.push(row.into_any_element());
            if open && depth < 16 {
                self.debug_variable_rows(session, child, depth + 1, rows, cx);
            }
        }
    }

    /// The Debug Console panel: program output plus an expression input.
    pub(super) fn render_debug_console(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = Theme::global(cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(div().flex_1().min_h_0().child(self.debug_console.clone()))
            .child(
                div()
                    .flex_none()
                    .px_2()
                    .py_1()
                    .flex()
                    .items_center()
                    .gap_1()
                    .border_t_1()
                    .border_color(t.c("panel.border"))
                    .child(icon("chevronRight", px(14.), t.c("icon.foreground")))
                    .child(div().flex_1().child(self.debug_input.clone())),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod container_tests {
    use crate::workbench::settings_page::view_tests::workbench;

    fn docker(args: &[&str]) -> String {
        let out = std::process::Command::new("docker").args(args).output().expect("docker");
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    /// "Start Debugging in Container" on a project with Visual Studio's Dockerfile: the image builds, the
    /// container starts, the debugger attaches to the app in it, and stopping removes the container.
    /// Builds a Docker image (pulling the .NET images), so it only runs with BF_TEST_DOCKER=1.
    #[gpui::test]
    fn start_debugging_in_a_container(cx: &mut gpui::TestAppContext) {
        if std::env::var("BF_TEST_DOCKER").is_err() {
            println!("SKIPPED: set BF_TEST_DOCKER=1 (needs Docker and netcoredbg)");
            return;
        }
        let (guard, wb, cx) = workbench(cx, "container-f5");
        let root = guard.dir.join("sln");
        let app = root.join("Ticker");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("Ticker.csproj"), "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net10.0</TargetFramework><ImplicitUsings>enable</ImplicitUsings></PropertyGroup></Project>").unwrap();
        std::fs::write(app.join("Program.cs"), "var n = 0;\nwhile (true)\n{\n    n++;\n    Console.WriteLine($\"tick {n}\");\n    Thread.Sleep(500);\n}\n").unwrap();
        std::fs::write(
            app.join("Dockerfile"),
            "FROM mcr.microsoft.com/dotnet/runtime:10.0 AS base\nWORKDIR /app\n\nFROM mcr.microsoft.com/dotnet/sdk:10.0 AS build\nARG BUILD_CONFIGURATION=Release\nWORKDIR /src\nCOPY [\"Ticker/Ticker.csproj\", \"Ticker/\"]\nRUN dotnet restore \"Ticker/Ticker.csproj\"\nCOPY . .\nWORKDIR \"/src/Ticker\"\nRUN dotnet build \"Ticker.csproj\" -c $BUILD_CONFIGURATION -o /app/build\n\nFROM base AS final\nWORKDIR /app\nCOPY --from=build /app/build .\nENTRYPOINT [\"dotnet\", \"Ticker.dll\"]\n",
        )
        .unwrap();

        wb.update_in(cx, |wb, window, cx| {
            wb.open_folder(root.clone(), window, cx);
            wb.debug_start_in_container(window, cx);
        });
        // Build, run, copy netcoredbg in, attach.
        let mut attached = false;
        for _ in 0..1200 {
            cx.run_until_parked();
            attached = wb.update(cx, |wb, _| wb.debug.as_ref().is_some_and(|s| s.attached && s.configured));
            if attached {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        let name = "byteforge-debug-ticker";
        assert!(attached, "the debugger attached to the app in the container");
        assert!(docker(&["ps", "--format", "{{.Names}}"]).lines().any(|n| n == name), "the container runs");
        assert!(wb.update(cx, |wb, _| wb.debug_container.is_some()));

        wb.update(cx, |wb, cx| wb.debug_stop(cx));
        let mut gone = false;
        for _ in 0..120 {
            cx.run_until_parked();
            gone = !docker(&["ps", "-a", "--format", "{{.Names}}"]).lines().any(|n| n == name);
            if gone {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        assert!(gone, "stopping the session removes the container");
        let _ = std::process::Command::new("docker").args(["rmi", "-f", name]).output();
        drop(guard);
    }
}
