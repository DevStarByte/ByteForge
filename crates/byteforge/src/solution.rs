//! Solution view: the projects of the workspace's `.sln`/`.slnx` in their solution folders, each with
//! its NuGet packages and project references. Building, running, testing and changing packages go
//! through the workbench; checking for package updates (`dotnet list package --outdated`) runs here.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use bf_core::Workspace;
use bf_core::nuget::{self, Outdated};
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
    /// Search NuGet for a package to add to the project.
    AddPackage(PathBuf),
    /// Change a package reference to this version.
    UpdatePackage(PathBuf, String, String),
    RemovePackage(PathBuf, String),
}

#[derive(Clone)]
enum Entry {
    Folder { key: String, name: String, count: usize, depth: usize },
    Project { project: Project, depth: usize },
    Group { key: String, label: &'static str, count: usize, depth: usize, project: PathBuf },
    Package { project: PathBuf, name: String, version: Option<String>, latest: Option<String>, depth: usize },
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
    /// The last update check: project → package id (lowercase) → newer version.
    outdated: Outdated,
    /// The update check is running, and why the last one failed.
    checking: bool,
    check_error: Option<String>,
    check_task: Option<Task<()>>,
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
            outdated: HashMap::new(),
            checking: false,
            check_error: None,
            check_task: None,
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
        self.outdated.clear();
        self.check_error = None;
        self.check_task = None;
        self.checking = false;
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
            out.push(Entry::Group { key: packages.clone(), label: "Packages", count: project.packages.len(), depth: depth + 1, project: project.path.clone() });
            if self.expanded.contains(&packages) {
                let newer = self.outdated.get(&project.path);
                out.extend(project.packages.iter().map(|p| Entry::Package {
                    project: project.path.clone(),
                    name: p.name.clone(),
                    version: p.version.clone(),
                    latest: newer.and_then(|n| n.get(&p.name.to_lowercase())).filter(|l| p.version.as_ref() != Some(*l)).cloned(),
                    depth: depth + 2,
                }));
            }
            let refs = format!("refs:{}", project.path.display());
            out.push(Entry::Group { key: refs.clone(), label: "Projects", count: project.references.len(), depth: depth + 1, project: project.path.clone() });
            if self.expanded.contains(&refs) {
                out.extend(project.references.iter().map(|r| Entry::Reference {
                    name: r.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                    path: r.clone(),
                    depth: depth + 2,
                }));
            }
        }
    }

    /// Checks every project for newer package versions (`dotnet list package --outdated`), for the whole
    /// solution at once when there is a solution file. Projects with updates get their Packages opened.
    pub fn check_updates(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.root() else { return };
        if self.checking {
            return;
        }
        if !self.loaded {
            self.load(cx);
        }
        self.checking = true;
        self.check_error = None;
        let preferred = self.preferred.clone();
        self.check_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let solution = solution::load(&root, preferred.as_deref());
                    let targets: Vec<PathBuf> = match solution.path {
                        Some(path) => vec![path],
                        None => solution.root.all_projects().into_iter().filter(|p| p.exists).map(|p| p.path.clone()).collect(),
                    };
                    let mut all = Outdated::new();
                    for target in targets {
                        all.extend(nuget::outdated(&root, &target)?);
                    }
                    anyhow::Ok(all)
                })
                .await;
            let _ = this.update(cx, |v, cx| {
                v.checking = false;
                match result {
                    Ok(outdated) => {
                        for (project, packages) in &outdated {
                            if !packages.is_empty() {
                                v.collapsed.remove(&project_key(project));
                                v.expanded.insert(format!("packages:{}", project.display()));
                            }
                        }
                        v.outdated = outdated;
                    }
                    Err(e) => v.check_error = Some(format!("{e:#}")),
                }
                v.rebuild();
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// A package was updated or removed: its update hint no longer applies.
    pub fn package_changed(&mut self, project: &Path, id: &str) {
        if let Some(packages) = self.outdated.get_mut(project) {
            packages.remove(&id.to_lowercase());
        }
    }

    fn update_count(&self) -> usize {
        self.outdated.values().map(HashMap::len).sum()
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
        let summary = match (self.checking, &self.check_error, self.update_count()) {
            (true, _, _) => format!("{summary} · checking for updates…"),
            (_, Some(_), _) => format!("{summary} · update check failed"),
            (_, _, 0) => summary,
            (_, _, 1) => format!("{summary} · 1 update"),
            (_, _, n) => format!("{summary} · {n} updates"),
        };
        let check_tip = match &self.check_error {
            Some(e) => format!("Check for Package Updates\n\nLast check failed: {e}"),
            None => "Check for Package Updates".to_owned(),
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
                        Entry::Group { key, label, count, depth, project } => {
                            let open = view.expanded.contains(&key);
                            let (e, add) = (entity.clone(), entity.clone());
                            let is_packages = label == "Packages";
                            row(depth)
                                .when(count > 0, |d| d.child(chevron(open)))
                                .when(count == 0, |d| d.child(blank()))
                                .child(icon(if label == "Packages" { "layers" } else { "fileProject" }, px(14.), t.c("icon.foreground")))
                                .child(div().whitespace_nowrap().text_color(if count == 0 { t.c("foreground.muted") } else { t.c("foreground") }).child(label))
                                .child(detail(count.to_string()))
                                .when(is_packages, |d| {
                                    d.child(div().invisible().group_hover("solution-row", |s| s.visible()).child(small("add", "add", "Add Package…").on_click(move |_, _, cx| {
                                        cx.stop_propagation();
                                        add.update(cx, |_, cx| cx.emit(SolutionEvent::AddPackage(project.clone())))
                                    })))
                                })
                                .when(count > 0, |d| d.on_click(move |_, _, cx| e.update(cx, |v, cx| v.toggle_expanded(key.clone(), cx))))
                                .into_any_element()
                        }
                        Entry::Package { project, name, version, latest, depth } => {
                            let current = version.clone().unwrap_or_default();
                            let (e1, e2) = (entity.clone(), entity.clone());
                            let (p1, p2, n1, n2) = (project.clone(), project, name.clone(), name.clone());
                            let newer = latest.clone();
                            let update_color = t.c_or("solution.updateForeground", 0x3794ffff);
                            row(depth)
                                .child(blank())
                                .child(icon("layers", px(12.), if latest.is_some() { update_color } else { t.c("foreground.muted") }))
                                .child(div().whitespace_nowrap().child(name))
                                .child(match &latest {
                                    Some(l) => div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_size(px(11.))
                                        .text_color(update_color)
                                        .child(format!("{current} → {l}")),
                                    None => detail(current),
                                })
                                .child(
                                    div()
                                        .flex()
                                        .gap_1()
                                        .invisible()
                                        .group_hover("solution-row", |s| s.visible())
                                        .when_some(newer, |d, l| {
                                            let tip: gpui::SharedString = format!("Update to {l}").into();
                                            d.child(
                                                div()
                                                    .id("update")
                                                    .p(px(2.))
                                                    .rounded(px(3.))
                                                    .cursor_pointer()
                                                    .hover(|s| s.bg(t.c("toolbar.hoverBackground")))
                                                    .tooltip(tooltip(tip))
                                                    .child(icon("arrowUp", px(14.), update_color))
                                                    .on_click(move |_, _, cx| {
                                                        cx.stop_propagation();
                                                        e1.update(cx, |_, cx| cx.emit(SolutionEvent::UpdatePackage(p1.clone(), n1.clone(), l.clone())))
                                                    }),
                                            )
                                        })
                                        .child(small("remove", "trash", "Remove Package").on_click(move |_, _, cx| {
                                            cx.stop_propagation();
                                            e2.update(cx, |_, cx| cx.emit(SolutionEvent::RemovePackage(p2.clone(), n2.clone())))
                                        })),
                                )
                                .into_any_element()
                        }
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
                    .when(self.solution.as_ref().is_some_and(|s| s.root.project_count() > 0), |d| {
                        d.child(
                            div()
                                .id("solution-updates")
                                .p(px(3.))
                                .rounded(px(4.))
                                .cursor_pointer()
                                .hover(|s| s.bg(t.c("toolbar.hoverBackground")))
                                .tooltip(tooltip(check_tip))
                                .child(icon("arrowUp", icon_size, if self.update_count() > 0 { t.c_or("solution.updateForeground", 0x3794ffff) } else { t.c("icon.foreground") }))
                                .on_click(cx.listener(|v, _, _, cx| v.check_updates(cx))),
                        )
                    })
                    .child(tool("solution-refresh", "refresh", "Reload").on_click(cx.listener(|v, _, _, cx| v.load(cx)))),
            )
            .child(list)
            .into_any_element()
    }
}
