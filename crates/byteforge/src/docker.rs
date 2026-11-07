//! Docker view: the workspace's Dockerfiles and Compose files, and the engine's containers and images,
//! with the usual actions (build, compose up/down, start/stop/restart, logs, shell, run, remove).
//! Long-running and interactive commands go to a terminal (the workbench runs them); quick ones run on a
//! background thread here. The engine state is polled while the view is visible.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use bf_core::Workspace;
use bf_core::docker::{self, Container, Image, Snapshot};
use gpui::{Context, EventEmitter, PromptLevel, Task, Window, div, prelude::*, px, uniform_list};

use crate::assets::icon;
use crate::theme::Theme;
use crate::workbench::tooltip;

pub enum DockerEvent {
    OpenFile(PathBuf),
    /// Run `args` (a full command line, e.g. `["docker", "logs", "-f", id]`) in a new terminal named `title`.
    RunInTerminal { title: String, args: Vec<String> },
    Build(PathBuf),
    Compose(PathBuf, ComposeVerb),
    Log(String),
    /// The number of running containers changed (the activity bar badge).
    StatusChanged,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ComposeVerb {
    Up,
    Down,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Section {
    Files,
    Containers,
    Images,
}

#[derive(Clone)]
enum Entry {
    Header { section: Section, count: usize },
    Group(String),
    File { path: PathBuf, compose: bool },
    Container { container: Container, grouped: bool },
    Image(Image),
    Note(String),
}

pub struct DockerView {
    workspace: Workspace,
    snapshot: Snapshot,
    dockerfiles: Vec<PathBuf>,
    compose_files: Vec<PathBuf>,
    entries: Vec<Entry>,
    collapsed: HashSet<Section>,
    /// `None` until the first check; `Some(false)` when the `docker` CLI isn't installed.
    cli: Option<bool>,
    /// Why the engine couldn't be queried (usually: the daemon isn't running).
    engine_error: Option<String>,
    busy: Option<String>,
    visible: bool,
    refresh_task: Option<Task<()>>,
    op_task: Option<Task<()>>,
    _poll: Task<()>,
}

impl EventEmitter<DockerEvent> for DockerView {}

impl DockerView {
    pub fn new(workspace: Workspace, cx: &mut Context<Self>) -> Self {
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(4)).await;
                let Ok(visible) = this.update(cx, |v, _| v.visible) else { break };
                if visible && this.update(cx, |v, cx| v.refresh(cx)).is_err() {
                    break;
                }
            }
        });
        Self {
            workspace,
            snapshot: Snapshot::default(),
            dockerfiles: Vec::new(),
            compose_files: Vec::new(),
            entries: Vec::new(),
            collapsed: HashSet::new(),
            cli: None,
            engine_error: None,
            busy: None,
            visible: false,
            refresh_task: None,
            op_task: None,
            _poll: poll,
        }
    }

    /// Called every frame by the workbench; refreshes right away when the view becomes visible.
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if visible && !self.visible {
            self.visible = true;
            self.refresh(cx);
        }
        self.visible = visible;
    }

    pub fn running_count(&self) -> usize {
        self.snapshot.containers.iter().filter(|c| c.is_running()).count()
    }

    /// Files in the workspace changed: rescan (only while visible; showing the view rescans anyway).
    pub fn files_changed(&mut self, cx: &mut Context<Self>) {
        if self.visible {
            self.refresh(cx);
        }
    }

    /// Re-reads the workspace's Docker files and the engine's containers and images in the background.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let workspace = self.workspace.clone();
        let check_cli = self.cli != Some(true);
        self.refresh_task = Some(cx.spawn(async move |this, cx| {
            let (files, cli, snapshot) = cx
                .background_executor()
                .spawn(async move {
                    let files = docker::docker_files(&workspace.files(), bf_syntax::is_dockerfile_name);
                    let cli = if check_cli { docker::cli_available() } else { true };
                    let snapshot = cli.then(docker::snapshot);
                    (files, cli, snapshot)
                })
                .await;
            let _ = this.update(cx, |v, cx| {
                let running = v.running_count();
                (v.dockerfiles, v.compose_files) = files;
                v.cli = Some(cli);
                match snapshot {
                    Some(Ok(snapshot)) => {
                        v.snapshot = snapshot;
                        v.engine_error = None;
                    }
                    Some(Err(e)) => {
                        v.snapshot = Snapshot::default();
                        v.engine_error = Some(e.to_string());
                    }
                    None => v.snapshot = Snapshot::default(),
                }
                v.rebuild();
                if v.running_count() != running {
                    cx.emit(DockerEvent::StatusChanged);
                }
                cx.notify();
            });
        }));
    }

    fn rebuild(&mut self) {
        let mut entries = Vec::new();
        let files = self.dockerfiles.len() + self.compose_files.len();
        entries.push(Entry::Header { section: Section::Files, count: files });
        if !self.collapsed.contains(&Section::Files) {
            if files == 0 {
                entries.push(Entry::Note("No Dockerfile yet: run \"Docker: Add Docker Support to Project…\"".into()));
            }
            entries.extend(self.compose_files.iter().map(|p| Entry::File { path: p.clone(), compose: true }));
            entries.extend(self.dockerfiles.iter().map(|p| Entry::File { path: p.clone(), compose: false }));
        }
        if self.cli == Some(true) && self.engine_error.is_none() {
            entries.push(Entry::Header { section: Section::Containers, count: self.snapshot.containers.len() });
            if !self.collapsed.contains(&Section::Containers) {
                let mut group: Option<&String> = None;
                for c in &self.snapshot.containers {
                    if c.compose_project.is_some() && c.compose_project.as_ref() != group {
                        group = c.compose_project.as_ref();
                        entries.push(Entry::Group(group.cloned().unwrap_or_default()));
                    }
                    entries.push(Entry::Container { container: c.clone(), grouped: c.compose_project.is_some() });
                }
            }
            entries.push(Entry::Header { section: Section::Images, count: self.snapshot.images.len() });
            if !self.collapsed.contains(&Section::Images) {
                entries.extend(self.snapshot.images.iter().cloned().map(Entry::Image));
            }
        }
        self.entries = entries;
    }

    fn toggle(&mut self, section: Section, cx: &mut Context<Self>) {
        if !self.collapsed.remove(&section) {
            self.collapsed.insert(section);
        }
        self.rebuild();
        cx.notify();
    }

    /// Runs a quick docker command in the background, then refreshes.
    fn run_op(&mut self, label: String, op: impl FnOnce() -> anyhow::Result<String> + Send + 'static, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(label.clone());
        cx.notify();
        self.op_task = Some(cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { op() }).await;
            let _ = this.update(cx, |v, cx| {
                v.busy = None;
                match result {
                    Ok(out) => {
                        let out = out.trim();
                        cx.emit(DockerEvent::Log(if out.is_empty() { format!("docker {label}: done") } else { format!("docker {label}: {out}") }));
                    }
                    Err(e) => cx.emit(DockerEvent::Log(format!("docker {label} failed: {e}"))),
                }
                v.refresh(cx);
                cx.notify();
            });
        }));
    }

    fn container_op(&mut self, verb: &'static str, c: &Container, cx: &mut Context<Self>) {
        let id = c.id.clone();
        let op = move || match verb {
            "start" => docker::start(&id),
            "stop" => docker::stop(&id),
            "restart" => docker::restart(&id),
            "unpause" => docker::unpause(&id),
            _ => docker::remove_container(&id),
        };
        self.run_op(format!("{verb} {}", c.name), op, cx);
    }

    fn remove_container(&mut self, c: Container, window: &mut Window, cx: &mut Context<Self>) {
        let detail = if c.is_running() { "The container is running; it will be stopped first." } else { "Its file system changes are lost (volumes are kept)." };
        let rx = window.prompt(PromptLevel::Warning, &format!("Remove container '{}'?", c.name), Some(detail), &["Remove", "Cancel"], cx);
        cx.spawn(async move |this, cx| {
            if let Ok(0) = rx.await {
                let _ = this.update(cx, |v, cx| v.container_op("rm", &c, cx));
            }
        })
        .detach();
    }

    fn remove_image(&mut self, image: Image, window: &mut Window, cx: &mut Context<Self>) {
        let reference = image.reference();
        let rx = window.prompt(PromptLevel::Warning, &format!("Remove image '{reference}'?"), Some("Images used by a container can't be removed."), &["Remove", "Cancel"], cx);
        cx.spawn(async move |this, cx| {
            if let Ok(0) = rx.await {
                let _ = this.update(cx, |v, cx| {
                    let r = reference.clone();
                    v.run_op(format!("rmi {reference}"), move || docker::remove_image(&r), cx)
                });
            }
        })
        .detach();
    }

    /// `docker system prune` after confirmation (stopped containers, dangling images, unused networks).
    pub fn prune(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = window.prompt(
            PromptLevel::Warning,
            "Remove all stopped containers, unused networks, dangling images and the build cache?",
            Some("Volumes are kept. This cannot be undone."),
            &["Prune", "Cancel"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if let Ok(0) = rx.await {
                let _ = this.update(cx, |v, cx| v.run_op("system prune".into(), docker::prune, cx));
            }
        })
        .detach();
    }
}

fn terminal(title: String, args: &[&str]) -> DockerEvent {
    DockerEvent::RunInTerminal { title, args: args.iter().map(|a| (*a).to_owned()).collect() }
}

impl Render for DockerView {
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
        let engine = match (self.cli, &self.engine_error) {
            (None, _) => "Looking for Docker…".to_owned(),
            (Some(false), _) => "Docker isn't installed (no docker command found).".to_owned(),
            (Some(true), Some(_)) => "The Docker engine isn't reachable. Start it (e.g. systemctl start docker) or check your Docker context.".to_owned(),
            (Some(true), None) => {
                let running = self.running_count();
                format!("{running} running · {} containers · {} images", self.snapshot.containers.len(), self.snapshot.images.len())
            }
        };

        let entity = cx.entity();
        let row_h = t.n("list.itemHeight", 22.0);
        let root = self.workspace.root().unwrap_or_default();
        let list = uniform_list("docker-entries", self.entries.len(), move |range, _, cx| {
            let view = entity.read(cx);
            let t = Theme::global(cx);
            range
                .map(|ix| {
                    let entry = view.entries[ix].clone();
                    let entity = entity.clone();
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
                    let actions = || div().flex().gap_1().invisible().group_hover("docker-row", |s| s.visible());
                    let row = |indent: f32| {
                        div()
                            .id(ix)
                            .group("docker-row")
                            .w_full()
                            .h(px(row_h))
                            .pl(px(indent))
                            .pr_3()
                            .flex()
                            .items_center()
                            .gap(t.m("explorer.itemSpacing", 6.0))
                            .text_size(t.m("list.fontSize", 13.0))
                            .hover(|s| s.bg(t.c("list.hoverBackground")))
                    };
                    let detail = |text: String| div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_size(px(11.)).text_color(t.c("foreground.muted")).child(text);
                    match entry {
                        Entry::Header { section, count } => {
                            let collapsed = view.collapsed.contains(&section);
                            let title = match section {
                                Section::Files => "WORKSPACE FILES",
                                Section::Containers => "CONTAINERS",
                                Section::Images => "IMAGES",
                            };
                            div()
                                .id(ix)
                                .w_full()
                                .h(px(row_h))
                                .px_2()
                                .flex()
                                .items_center()
                                .gap_1()
                                .cursor_pointer()
                                .text_size(t.m("sideBarSectionHeader.fontSize", 11.0))
                                .font_weight(t.weight("sideBarSectionHeader.weight", gpui::FontWeight::BOLD))
                                .text_color(t.c("sideBarSectionHeader.foreground"))
                                .child(icon(if collapsed { "chevronRight" } else { "chevronDown" }, px(14.), t.c("icon.foreground")))
                                .child(div().flex_1().child(title))
                                .child(div().px(px(6.)).rounded(px(9.)).text_size(px(11.)).bg(t.bg("badge.background")).text_color(t.c("badge.foreground")).child(count.to_string()))
                                .on_click(move |_, _, cx| entity.update(cx, |v, cx| v.toggle(section, cx)))
                                .into_any_element()
                        }
                        Entry::Group(project) => row(20.)
                            .text_color(t.c("foreground.muted"))
                            .child(icon("layers", px(14.), t.c("icon.docker")))
                            .child(div().whitespace_nowrap().child(format!("{project} (compose)")))
                            .into_any_element(),
                        Entry::Note(text) => row(24.).child(div().whitespace_nowrap().text_size(px(12.)).text_color(t.c("foreground.muted")).child(text)).into_any_element(),
                        Entry::File { path, compose } => {
                            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                            let dir = path.parent().and_then(|p| p.strip_prefix(&root).ok()).map(|p| p.display().to_string()).unwrap_or_default();
                            let (e1, e2, e3) = (entity.clone(), entity.clone(), entity.clone());
                            let (p1, p2, p3) = (path.clone(), path.clone(), path.clone());
                            let buttons = if compose {
                                actions()
                                    .child(small("up", "play", "Compose Up (build and start in the background)").on_click(move |_, _, cx| {
                                        cx.stop_propagation();
                                        e1.update(cx, |_, cx| cx.emit(DockerEvent::Compose(p1.clone(), ComposeVerb::Up)))
                                    }))
                                    .child(small("down", "stop", "Compose Down (stop and remove the containers)").on_click(move |_, _, cx| {
                                        cx.stop_propagation();
                                        e2.update(cx, |_, cx| cx.emit(DockerEvent::Compose(p2.clone(), ComposeVerb::Down)))
                                    }))
                            } else {
                                actions().child(small("build", "build", "Build Image").on_click(move |_, _, cx| {
                                    cx.stop_propagation();
                                    e1.update(cx, |_, cx| cx.emit(DockerEvent::Build(p1.clone())))
                                }))
                            };
                            row(24.)
                                .cursor_pointer()
                                .child(icon(if compose { "layers" } else { "docker" }, t.m("explorer.iconSize", 15.0), t.c("icon.docker")))
                                .child(div().whitespace_nowrap().child(name))
                                .child(detail(dir))
                                .child(buttons)
                                .on_click(move |_, _, cx| e3.update(cx, |_, cx| cx.emit(DockerEvent::OpenFile(p3.clone()))))
                                .into_any_element()
                        }
                        Entry::Container { container: c, grouped } => {
                            let state_color = match c.state.as_str() {
                                "running" => t.c_or("docker.runningForeground", 0x3fb950ff),
                                "paused" | "restarting" | "created" => t.c_or("docker.pausedForeground", 0xd29922ff),
                                _ => t.c("foreground.muted"),
                            };
                            let label = match &c.compose_service {
                                Some(service) if grouped => service.clone(),
                                _ => c.name.clone(),
                            };
                            let mut tip = format!("{}\n{}\nimage: {}\nid: {}", c.name, c.status, c.image, c.id);
                            if !c.ports.is_empty() {
                                tip.push_str(&format!("\nports: {}", c.ports));
                            }
                            let mut buttons = actions();
                            let (e, cc) = (entity.clone(), c.clone());
                            buttons = buttons.child(small("logs", "file", "Logs (follow in a terminal)").on_click(move |_, _, cx| {
                                e.update(cx, |_, cx| cx.emit(terminal(format!("logs: {}", cc.name), &["docker", "logs", "--follow", "--tail", "500", &cc.id])))
                            }));
                            if c.is_running() {
                                let (e, cc) = (entity.clone(), c.clone());
                                buttons = buttons.child(small("shell", "terminal", "Open a Shell in the Container").on_click(move |_, _, cx| {
                                    e.update(cx, |_, cx| {
                                        // bash when the image has it, else sh (Alpine, distroless debug images, …).
                                        cx.emit(terminal(format!("shell: {}", cc.name), &["docker", "exec", "-it", &cc.id, "sh", "-c", "command -v bash >/dev/null && exec bash || exec sh"]))
                                    })
                                }));
                                let (e, cc) = (entity.clone(), c.clone());
                                buttons = buttons.child(small("restart", "refresh", "Restart").on_click(move |_, _, cx| e.update(cx, |v, cx| v.container_op("restart", &cc, cx))));
                                let (e, cc) = (entity.clone(), c.clone());
                                buttons = buttons.child(small("stop", "stop", "Stop").on_click(move |_, _, cx| e.update(cx, |v, cx| v.container_op("stop", &cc, cx))));
                            } else {
                                let (e, cc) = (entity.clone(), c.clone());
                                let verb = if c.is_paused() { "unpause" } else { "start" };
                                buttons = buttons.child(small("start", "play", if c.is_paused() { "Unpause" } else { "Start" }).on_click(move |_, _, cx| e.update(cx, |v, cx| v.container_op(verb, &cc, cx))));
                            }
                            let (e, cc) = (entity.clone(), c.clone());
                            buttons = buttons.child(small("remove", "trash", "Remove").on_click(move |_, window, cx| e.update(cx, |v, cx| v.remove_container(cc.clone(), window, cx))));
                            row(if grouped { 38. } else { 24. })
                                .tooltip(tooltip(tip))
                                .child(icon("breakpoint", px(12.), state_color))
                                .child(div().whitespace_nowrap().child(label))
                                .child(detail(if c.is_running() && !c.ports.is_empty() { c.ports.clone() } else { c.status.clone() }))
                                .child(buttons)
                                .into_any_element()
                        }
                        Entry::Image(image) => {
                            let reference = image.reference();
                            let (e1, e2) = (entity.clone(), entity.clone());
                            let (r1, i2) = (reference.clone(), image.clone());
                            row(24.)
                                .tooltip(tooltip(format!("{reference}\nid: {}\nsize: {}\ncreated {}", image.id, image.size, image.created)))
                                .child(icon("layers", px(14.), t.c("icon.docker")))
                                .child(div().whitespace_nowrap().child(if image.repository == "<none>" { "<none>".to_owned() } else { reference.clone() }))
                                .child(detail(image.size.clone()))
                                .child(
                                    actions()
                                        .child(small("run", "play", "Run (interactive, removed on exit, ports published)").on_click(move |_, _, cx| {
                                            e1.update(cx, |_, cx| cx.emit(terminal(format!("run: {r1}"), &["docker", "run", "--rm", "-it", "--publish-all", &r1])))
                                        }))
                                        .child(small("remove", "trash", "Remove Image").on_click(move |_, window, cx| e2.update(cx, |v, cx| v.remove_image(i2.clone(), window, cx)))),
                                )
                                .into_any_element()
                        }
                    }
                })
                .collect()
        })
        .flex_1();

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
                    .child(icon("docker", icon_size, t.c("icon.docker")))
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_size(px(12.)).text_color(muted).child(self.busy.as_ref().map_or(engine, |b| format!("docker {b}…"))))
                    .child(tool("docker-refresh", "refresh", "Refresh").on_click(cx.listener(|v, _, _, cx| v.refresh(cx))))
                    .child(tool("docker-prune", "trash", "System Prune…").on_click(cx.listener(|v, _, window, cx| v.prune(window, cx)))),
            )
            .when_some(self.engine_error.clone(), |d, e| d.child(div().px_3().text_size(px(11.)).text_color(t.c("errorForeground")).child(e)))
            .child(list)
            .into_any_element()
    }
}
