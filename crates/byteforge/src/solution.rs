//! Solution view: the projects of the workspace's `.sln`/`.slnx` in their solution folders, each with
//! its NuGet packages and project references. Building, running and testing go through the workbench.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use bf_core::Workspace;
use bf_core::solution::{self, Folder, Project, Solution};
use gpui::{Context, EventEmitter, Task, Window, div, prelude::*, px, uniform_list};

use crate::assets::icon;
use crate::theme::Theme;
use crate::workbench::tooltip;

pub enum SolutionEvent {
    OpenFile(PathBuf),
    Build(PathBuf),
    Run(PathBuf),
    Test(PathBuf),
    Reveal(PathBuf),
}

#[derive(Clone)]
enum Entry {
    Folder { key: String, name: String, count: usize, depth: usize },
    Project { project: Project, depth: usize },
    Group { key: String, label: &'static str, count: usize, depth: usize },
    Package { name: String, version: Option<String>, depth: usize },
    Reference { name: String, path: PathBuf, depth: usize },
}

pub struct SolutionView {
    workspace: Workspace,
    /// Read from settings by the workbench: a configured `dotnet.project` that is a solution wins.
    preferred: Option<String>,
    solution: Option<Solution>,
    /// Folders and projects the user closed (open by default).
    collapsed: HashSet<String>,
    /// Dependency groups the user opened (closed by default).
    expanded: HashSet<String>,
    entries: Vec<Entry>,
    loaded: bool,
    visible: bool,
    load_task: Option<Task<()>>,
}

impl EventEmitter<SolutionEvent> for SolutionView {}

impl SolutionView {
    pub fn new(workspace: Workspace, _: &mut Context<Self>) -> Self {
        Self {
            workspace,
            preferred: None,
            solution: None,
            collapsed: HashSet::new(),
            expanded: HashSet::new(),
            entries: Vec::new(),
            loaded: false,
            visible: false,
            load_task: None,
        }
    }

    pub fn set_preferred(&mut self, preferred: Option<String>) {
        self.preferred = preferred;
    }

    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if visible && !self.visible && !self.loaded {
            self.load(cx);
        }
        self.visible = visible;
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    pub fn open_folder(&mut self, cx: &mut Context<Self>) {
        self.solution = None;
        self.loaded = false;
        self.collapsed.clear();
        self.expanded.clear();
        self.rebuild();
        if self.visible {
            self.load(cx);
        }
        cx.notify();
    }

    pub fn load(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.root() else { return };
        let preferred = self.preferred.clone();
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let solution = cx.background_executor().spawn(async move { solution::load(&root, preferred.as_deref()) }).await;
            let _ = this.update(cx, |v, cx| {
                v.solution = Some(solution);
                v.loaded = true;
                v.rebuild();
                cx.notify();
            });
        }));
    }

    fn rebuild(&mut self) {
        let mut entries = Vec::new();
        if let Some(solution) = &self.solution {
            self.push_folder(&solution.root, "", 0, &mut entries);
        }
        self.entries = entries;
    }

    fn push_folder(&self, folder: &Folder, path: &str, depth: usize, out: &mut Vec<Entry>) {
        for sub in &folder.folders {
            let key = format!("folder:{path}/{}", sub.name);
            out.push(Entry::Folder { key: key.clone(), name: sub.name.clone(), count: sub.project_count(), depth });
            if !self.collapsed.contains(&key) {
                self.push_folder(sub, &format!("{path}/{}", sub.name), depth + 1, out);
            }
        }
        for project in &folder.projects {
            out.push(Entry::Project { project: project.clone(), depth });
            let key = project_key(&project.path);
            if self.collapsed.contains(&key) || !project.exists {
                continue;
            }
            let packages = format!("packages:{}", project.path.display());
            out.push(Entry::Group { key: packages.clone(), label: "Packages", count: project.packages.len(), depth: depth + 1 });
            if self.expanded.contains(&packages) {
                out.extend(project.packages.iter().map(|p| Entry::Package { name: p.name.clone(), version: p.version.clone(), depth: depth + 2 }));
            }
            let refs = format!("refs:{}", project.path.display());
            out.push(Entry::Group { key: refs.clone(), label: "Projects", count: project.references.len(), depth: depth + 1 });
            if self.expanded.contains(&refs) {
                out.extend(project.references.iter().map(|r| Entry::Reference {
                    name: r.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                    path: r.clone(),
                    depth: depth + 2,
                }));
            }
        }
    }

    fn toggle_collapsed(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.collapsed.remove(&key) {
            self.collapsed.insert(key);
        }
        self.rebuild();
        cx.notify();
    }

    fn toggle_expanded(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.expanded.remove(&key) {
            self.expanded.insert(key);
        }
        self.rebuild();
        cx.notify();
    }
}

fn project_key(path: &Path) -> String {
    format!("project:{}", path.display())
}

impl Render for SolutionView {
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
        let summary = match &self.solution {
            _ if self.workspace.root().is_none() => "Open a folder with a .NET solution or project.".to_owned(),
            None => "Reading the solution…".to_owned(),
            Some(s) if s.root.project_count() == 0 => "No .sln, .slnx or .csproj found.".to_owned(),
            Some(s) => {
                let n = s.root.project_count();
                let what = if n == 1 { "project".to_owned() } else { format!("{n} projects") };
                match &s.path {
                    Some(_) => format!("{} · {what}", s.name),
                    None => format!("{what} (no solution file)"),
                }
            }
        };
        let solution_path = self.solution.as_ref().and_then(|s| s.path.clone());

        let entity = cx.entity();
        let row_h = t.n("list.itemHeight", 22.0);
        let list = uniform_list("solution-entries", self.entries.len(), move |range, _, cx| {
            let view = entity.read(cx);
            let t = Theme::global(cx);
            range
                .map(|ix| {
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
                    let row = |depth: usize| {
                        div()
                            .id(ix)
                            .group("solution-row")
                            .w_full()
                            .h(px(row_h))
                            .pl(px(6. + depth as f32 * 14.))
                            .pr_3()
                            .flex()
                            .items_center()
                            .gap(t.m("explorer.itemSpacing", 6.0))
                            .text_size(t.m("list.fontSize", 13.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(t.c("list.hoverBackground")))
                    };
                    let detail = |text: String| div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_size(px(11.)).text_color(t.c("foreground.muted")).child(text);
                    let chevron = |open: bool| icon(if open { "chevronDown" } else { "chevronRight" }, px(14.), t.c("icon.foreground"));
                    let blank = || div().w(px(14.)).flex_none();
                    match view.entries[ix].clone() {
                        Entry::Folder { key, name, count, depth } => {
                            let open = !view.collapsed.contains(&key);
                            let e = entity.clone();
                            row(depth)
                                .child(chevron(open))
                                .child(icon(if open { "folderOpen" } else { "folder" }, px(14.), t.c_or("icon.folder", 0xdcb67aff)))
                                .child(div().whitespace_nowrap().child(name))
                                .child(detail(if count == 1 { "1 project".into() } else { format!("{count} projects") }))
                                .on_click(move |_, _, cx| e.update(cx, |v, cx| v.toggle_collapsed(key.clone(), cx)))
                                .into_any_element()
                        }
                        Entry::Project { project, depth } => {
                            let key = project_key(&project.path);
                            let open = !view.collapsed.contains(&key) && project.exists;
                            let path = project.path.clone();
                            let emit = |event: fn(PathBuf) -> SolutionEvent| {
                                let (e, path) = (entity.clone(), path.clone());
                                move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut gpui::App| {
                                    cx.stop_propagation();
                                    e.update(cx, |_, cx| cx.emit(event(path.clone())));
                                }
                            };
                            let buttons = div()
                                .flex()
                                .gap_1()
                                .invisible()
                                .group_hover("solution-row", |s| s.visible())
                                .when(project.exists, |d| d.child(small("build", "build", "Build").on_click(emit(SolutionEvent::Build))))
                                .when(project.is_app, |d| d.child(small("run", "play", "Run").on_click(emit(SolutionEvent::Run))))
                                .when(project.is_test, |d| d.child(small("test", "beaker", "Run Tests").on_click(emit(SolutionEvent::Test))))
                                .child(small("reveal", "files", "Reveal in Explorer").on_click(emit(SolutionEvent::Reveal)))
                                .when(project.exists, |d| d.child(small("open", "fileProject", "Open Project File").on_click(emit(SolutionEvent::OpenFile))));
                            let e = entity.clone();
                            let color = if project.exists { t.c_or("icon.project", 0xa074c4ff) } else { t.c_or("errorForeground", 0xf14c4cff) };
                            row(depth)
                                .tooltip(tooltip(project.path.display().to_string()))
                                .child(if project.exists { chevron(open).into_any_element() } else { blank().into_any_element() })
                                .child(icon("fileProject", px(14.), color))
                                .child(div().whitespace_nowrap().font_weight(gpui::FontWeight::SEMIBOLD).child(project.name.clone()))
                                .child(detail(project.summary()))
                                .child(buttons)
                                .on_click(move |_, _, cx| e.update(cx, |v, cx| v.toggle_collapsed(key.clone(), cx)))
                                .into_any_element()
                        }
                        Entry::Group { key, label, count, depth } => {
                            let open = view.expanded.contains(&key);
                            let e = entity.clone();
                            row(depth)
                                .when(count > 0, |d| d.child(chevron(open)))
                                .when(count == 0, |d| d.child(blank()))
                                .child(icon(if label == "Packages" { "layers" } else { "fileProject" }, px(14.), t.c("icon.foreground")))
                                .child(div().whitespace_nowrap().text_color(if count == 0 { t.c("foreground.muted") } else { t.c("foreground") }).child(label))
                                .child(detail(count.to_string()))
                                .when(count > 0, |d| d.on_click(move |_, _, cx| e.update(cx, |v, cx| v.toggle_expanded(key.clone(), cx))))
                                .into_any_element()
                        }
                        Entry::Package { name, version, depth } => row(depth)
                            .child(blank())
                            .child(icon("layers", px(12.), t.c("foreground.muted")))
                            .child(div().whitespace_nowrap().child(name))
                            .child(detail(version.unwrap_or_default()))
                            .into_any_element(),
                        Entry::Reference { name, path, depth } => {
                            let e = entity.clone();
                            row(depth)
                                .tooltip(tooltip(path.display().to_string()))
                                .child(blank())
                                .child(icon("fileProject", px(12.), t.c("foreground.muted")))
                                .child(div().whitespace_nowrap().child(name))
                                .on_click(move |_, _, cx| e.update(cx, |_, cx| cx.emit(SolutionEvent::OpenFile(path.clone()))))
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
                    .child(icon("fileSolution", icon_size, t.c_or("icon.solution", 0xb48ef0ff)))
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_size(px(12.)).text_color(muted).child(summary))
                    .when_some(solution_path, |d, path| {
                        let open = path.clone();
                        d.child(tool("solution-build", "build", "Build Solution").on_click(cx.listener(move |_, _, _, cx| cx.emit(SolutionEvent::Build(path.clone())))))
                            .child(tool("solution-open", "fileSolution", "Open Solution File").on_click(cx.listener(move |_, _, _, cx| cx.emit(SolutionEvent::OpenFile(open.clone())))))
                    })
                    .child(tool("solution-refresh", "refresh", "Reload").on_click(cx.listener(|v, _, _, cx| v.load(cx)))),
            )
            .child(list)
            .into_any_element()
    }
}
