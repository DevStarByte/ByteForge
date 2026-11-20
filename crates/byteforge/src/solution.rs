//! Solution view: the projects of the workspace's `.sln`/`.slnx` in their solution folders, each with
//! its dependencies (NuGet packages and project references) and its folders and files, like Visual
//! Studio's Solution Explorer: what an SDK-style project includes (no `bin`/`obj`, hidden folders or
//! other projects' folders), with generated and per-environment files nested under the file they belong to. Building, running, testing and changing packages go
//! through the workbench; checking for package updates (`dotnet list package --outdated`) runs here.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use bf_core::Workspace;
use bf_core::nuget::{self, Outdated};
use bf_core::solution::{self, Folder, Project, Solution};
use gpui::{Context, EventEmitter, Task, Window, div, prelude::*, px, uniform_list};

use crate::assets::icon;
use crate::explorer::file_icon_view;
use crate::theme::Theme;
use crate::workbench::tooltip;

pub enum SolutionEvent {
    OpenFile(PathBuf),
    Build(PathBuf),
    Run(PathBuf),
    /// `dotnet watch`: run with hot reload.
    Watch(PathBuf),
    Test(PathBuf),
    Reveal(PathBuf),
    /// Search NuGet for a package to add to the project.
    AddPackage(PathBuf),
    /// Change a package reference to this version.
    UpdatePackage(PathBuf, String, String),
    RemovePackage(PathBuf, String),
    /// Pick another version of an installed package (project, package id, installed version).
    ChangePackageVersion(PathBuf, String, String),
}

#[derive(Clone)]
enum Entry {
    Folder { key: String, name: String, count: usize, depth: usize },
    Project { project: Project, depth: usize },
    Group { key: String, label: &'static str, count: usize, depth: usize, project: PathBuf },
    Package { project: PathBuf, name: String, version: Option<String>, latest: Option<String>, depth: usize },
    Reference { name: String, path: PathBuf, depth: usize },
    /// The project's Dependencies node (packages and project references).
    Dependencies { key: String, count: usize, depth: usize },
    /// A folder or file of a project. `key` toggles it (folders, and files with nested files).
    Item { path: PathBuf, name: String, is_dir: bool, key: Option<String>, open: bool, depth: usize },
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
    /// The row under the mouse, which shows its action buttons. Kept here rather than as a hover style:
    /// a hover style that changes `display` makes gpui paint elements it never laid out.
    hovered: Option<usize>,
    /// The file in the active editor tab.
    active: Option<PathBuf>,
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
            hovered: None,
            active: None,
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
            let deps = format!("deps:{}", project.path.display());
            out.push(Entry::Dependencies { key: deps.clone(), count: project.packages.len() + project.references.len(), depth: depth + 1 });
            if self.expanded.contains(&deps) {
                self.push_dependencies(project, depth + 2, out);
            }
            if let Some(dir) = project.path.parent() {
                self.push_files(dir, true, depth + 1, out);
            }
        }
    }

    fn push_dependencies(&self, project: &Project, depth: usize, out: &mut Vec<Entry>) {
        {
            let packages = format!("packages:{}", project.path.display());
            out.push(Entry::Group { key: packages.clone(), label: "Packages", count: project.packages.len(), depth, project: project.path.clone() });
            if self.expanded.contains(&packages) {
                let newer = self.outdated.get(&project.path);
                out.extend(project.packages.iter().map(|p| Entry::Package {
                    project: project.path.clone(),
                    name: p.name.clone(),
                    version: p.version.clone(),
                    latest: newer.and_then(|n| n.get(&p.name.to_lowercase())).filter(|l| p.version.as_ref() != Some(*l)).cloned(),
                    depth: depth + 1,
                }));
            }
            let refs = format!("refs:{}", project.path.display());
            if project.references.is_empty() {
                return;
            }
            out.push(Entry::Group { key: refs.clone(), label: "Projects", count: project.references.len(), depth, project: project.path.clone() });
            if self.expanded.contains(&refs) {
                out.extend(project.references.iter().map(|r| Entry::Reference {
                    name: r.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                    path: r.clone(),
                    depth: depth + 1,
                }));
            }
        }
    }

    /// The folders and files of `dir` that belong to the project, folders first, with nested files
    /// (`Index.cshtml.cs`, `appsettings.Development.json`) under their parent file.
    fn push_files(&self, dir: &Path, project_root: bool, depth: usize, out: &mut Vec<Entry>) {
        let items: Vec<(PathBuf, String, bool)> = self
            .workspace
            .list_dir(dir)
            .into_iter()
            .filter_map(|(path, is_dir)| {
                let name = path.file_name()?.to_string_lossy().into_owned();
                project_item(&path, &name, is_dir, project_root).then_some((path, name, is_dir))
            })
            .collect();
        let files: HashSet<&str> = items.iter().filter(|i| !i.2).map(|i| i.1.as_str()).collect();
        let mut nested: HashMap<&str, Vec<&(PathBuf, String, bool)>> = HashMap::new();
        for item in items.iter().filter(|i| !i.2) {
            if let Some(parent) = nest_parent(&item.1, &files) {
                nested.entry(parent).or_default().push(item);
            }
        }
        for item in &items {
            if !item.2 && nest_parent(&item.1, &files).is_some() {
                continue;
            }
            self.push_item(item, &nested, depth, out);
        }
    }

    fn push_item(&self, (path, name, is_dir): &(PathBuf, String, bool), nested: &HashMap<&str, Vec<&(PathBuf, String, bool)>>, depth: usize, out: &mut Vec<Entry>) {
        let children = if *is_dir { None } else { nested.get(name.as_str()) };
        let key = (*is_dir || children.is_some()).then(|| format!("item:{}", path.display()));
        let open = key.as_ref().is_some_and(|k| self.expanded.contains(k));
        out.push(Entry::Item { path: path.clone(), name: name.clone(), is_dir: *is_dir, key, open, depth });
        if !open {
            return;
        }
        if *is_dir {
            self.push_files(path, false, depth + 1, out);
        } else if let Some(children) = children {
            for child in children {
                self.push_item(child, nested, depth + 1, out);
            }
        }
    }

    /// The editor's active file changed: it's highlighted when it's in the tree.
    pub fn set_active(&mut self, path: Option<&Path>, cx: &mut Context<Self>) {
        if self.active.as_deref() != path {
            self.active = path.map(Path::to_path_buf);
            cx.notify();
        }
    }

    /// Files were added, removed or renamed: re-read the open folders.
    pub fn files_changed(&mut self, cx: &mut Context<Self>) {
        if self.loaded {
            self.rebuild();
            cx.notify();
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
                                v.expanded.insert(format!("deps:{}", project.display()));
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

/// A row's name: keeps its width while there's room, then ends in "…" (before the muted detail gives way).
fn name_cell(name: String) -> gpui::Div {
    div().min_w_0().flex_shrink().overflow_hidden().whitespace_nowrap().text_ellipsis().child(name)
}

/// Whether a folder entry is part of the project, as SDK-style projects include files by default:
/// not `bin`/`obj` (at the project root), hidden folders, `node_modules`, folders of other projects,
/// the project file itself, solution files or `*.user` files.
fn project_item(path: &Path, name: &str, is_dir: bool, project_root: bool) -> bool {
    let lower = name.to_ascii_lowercase();
    if is_dir {
        if name.starts_with('.') || lower == "node_modules" || (project_root && (lower == "bin" || lower == "obj")) {
            return false;
        }
        return !std::fs::read_dir(path).map(|entries| entries.flatten().any(|e| is_project_file(&e.file_name().to_string_lossy()))).unwrap_or(false);
    }
    !(is_project_file(&lower) || lower.ends_with(".sln") || lower.ends_with(".slnx") || lower.ends_with(".user"))
}

fn is_project_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [".csproj", ".fsproj", ".vbproj", ".esproj", ".sqlproj"].iter().any(|ext| lower.ends_with(ext))
}

/// The file `name` is nested under, Visual Studio style: another file's name plus more extensions
/// (`Index.cshtml.cs` and `Counter.razor.css` under `Index.cshtml` and `Counter.razor`), and
/// per-environment settings (`appsettings.Development.json`, `web.Release.config`) under the base file.
fn nest_parent<'a>(name: &str, files: &HashSet<&'a str>) -> Option<&'a str> {
    // An existing file with an extension, plus more extensions: the longest such file wins.
    for (i, _) in name.match_indices('.').collect::<Vec<_>>().into_iter().rev() {
        let candidate = &name[..i];
        if i > 0 && candidate.contains('.')
            && !candidate.starts_with('.')
            && let Some(parent) = files.get(candidate)
        {
            return Some(parent);
        }
    }
    // base.Environment.json → base.json (also .config).
    let (first, last) = (name.find('.')?, name.rfind('.')?);
    let ext = &name[last..];
    if first > 0 && first < last && (ext.eq_ignore_ascii_case(".json") || ext.eq_ignore_ascii_case(".config")) {
        return files.get(format!("{}{ext}", &name[..first]).as_str()).copied();
    }
    None
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
            let hovered = view.hovered;
            let active = view.active.clone();
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
                            .on_hover({
                                let e = entity.clone();
                                move |&over: &bool, _, cx| {
                                    e.update(cx, |v, cx| {
                                        if over {
                                            v.hovered = Some(ix);
                                        } else if v.hovered == Some(ix) {
                                            v.hovered = None;
                                        }
                                        cx.notify();
                                    })
                                }
                            })
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
                    let detail = |text: String| div().flex_1().flex_basis(px(0.)).min_w_0().overflow_hidden().whitespace_nowrap().text_size(px(11.)).text_color(t.c("foreground.muted")).child(text);
                    let chevron = |open: bool| icon(if open { "chevronDown" } else { "chevronRight" }, px(14.), t.c("icon.foreground"));
                    let blank = || div().w(px(14.)).flex_none();
                    match view.entries[ix].clone() {
                        Entry::Folder { key, name, count, depth } => {
                            let open = !view.collapsed.contains(&key);
                            let e = entity.clone();
                            row(depth)
                                .child(chevron(open))
                                .child(icon(if open { "folderOpen" } else { "folder" }, px(14.), t.c_or("icon.folder", 0xdcb67aff)))
                                .child(name_cell(name))
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
                                .flex_none()
                                .when(hovered != Some(ix), |d| d.hidden())
                                .when(project.exists, |d| d.child(small("build", "build", "Build").on_click(emit(SolutionEvent::Build))))
                                .when(project.is_app, |d| d.child(small("run", "play", "Run").on_click(emit(SolutionEvent::Run))))
                                .when(project.is_app, |d| d.child(small("watch", "flame", "Run with Watch (Hot Reload)").on_click(emit(SolutionEvent::Watch))))
                                .when(project.is_test, |d| d.child(small("test", "beaker", "Run Tests").on_click(emit(SolutionEvent::Test))))
                                .child(small("reveal", "files", "Reveal in Explorer").on_click(emit(SolutionEvent::Reveal)))
                                .when(project.exists, |d| d.child(small("open", "fileProject", "Open Project File").on_click(emit(SolutionEvent::OpenFile))));
                            let e = entity.clone();
                            let color = if project.exists { t.c_or("icon.project", 0xa074c4ff) } else { t.c_or("errorForeground", 0xf14c4cff) };
                            row(depth)
                                .tooltip(tooltip(project.path.display().to_string()))
                                .child(if project.exists { chevron(open).into_any_element() } else { blank().into_any_element() })
                                .child(icon("fileProject", px(14.), color))
                                .child(name_cell(project.name.clone()).font_weight(gpui::FontWeight::SEMIBOLD))
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
                                .child(div().whitespace_nowrap().when(count == 0, |d| d.text_color(t.c("foreground.muted"))).child(label))
                                .child(detail(count.to_string()))
                                .when(is_packages, |d| {
                                    d.child(div().flex_none().when(hovered != Some(ix), |d| d.hidden()).child(small("add", "add", "Add Package…").on_click(move |_, _, cx| {
                                        cx.stop_propagation();
                                        add.update(cx, |_, cx| cx.emit(SolutionEvent::AddPackage(project.clone())))
                                    })))
                                })
                                .when(count > 0, |d| d.on_click(move |_, _, cx| e.update(cx, |v, cx| v.toggle_expanded(key.clone(), cx))))
                                .into_any_element()
                        }
                        Entry::Package { project, name, version, latest, depth } => {
                            let current = version.clone().unwrap_or_default();
                            let (e1, e2, e3) = (entity.clone(), entity.clone(), entity.clone());
                            let (p3, n3, v3) = (project.clone(), name.clone(), version.clone().unwrap_or_default());
                            let (p1, p2, n1, n2) = (project.clone(), project, name.clone(), name.clone());
                            let newer = latest.clone();
                            let update_color = t.c_or("solution.updateForeground", 0x3794ffff);
                            row(depth)
                                .child(blank())
                                .child(icon("layers", px(12.), if latest.is_some() { update_color } else { t.c("foreground.muted") }))
                                .child(name_cell(name))
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
                                        .flex_none()
                                        .gap_1()
                                        .when(hovered != Some(ix), |d| d.hidden())
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
                                        .child(small("version", "layers", "Change Version…").on_click(move |_, _, cx| {
                                            cx.stop_propagation();
                                            e3.update(cx, |_, cx| cx.emit(SolutionEvent::ChangePackageVersion(p3.clone(), n3.clone(), v3.clone())))
                                        }))
                                        .child(small("remove", "trash", "Remove Package").on_click(move |_, _, cx| {
                                            cx.stop_propagation();
                                            e2.update(cx, |_, cx| cx.emit(SolutionEvent::RemovePackage(p2.clone(), n2.clone())))
                                        })),
                                )
                                .into_any_element()
                        }
                        Entry::Dependencies { key, count, depth } => {
                            let open = view.expanded.contains(&key);
                            let e = entity.clone();
                            row(depth)
                                .child(chevron(open))
                                .child(icon("layers", px(14.), t.c("icon.foreground")))
                                .child(name_cell("Dependencies".into()))
                                .child(detail(count.to_string()))
                                .on_click(move |_, _, cx| e.update(cx, |v, cx| v.toggle_expanded(key.clone(), cx)))
                                .into_any_element()
                        }
                        Entry::Item { path, name, is_dir, key, open, depth } => {
                            let is_active = active.as_ref() == Some(&path);
                            let toggle = key.clone().map(|key| {
                                let e = entity.clone();
                                move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut gpui::App| {
                                    cx.stop_propagation();
                                    e.update(cx, |v, cx| v.toggle_expanded(key.clone(), cx))
                                }
                            });
                            let e = entity.clone();
                            row(depth)
                                .tooltip(tooltip(path.display().to_string()))
                                .child(match toggle.clone() {
                                    Some(toggle) => div().id("toggle").flex_none().child(chevron(open)).on_click(toggle).into_any_element(),
                                    None => blank().into_any_element(),
                                })
                                .child(div().flex_none().child(file_icon_view(&name, is_dir, open, px(14.), &t)))
                                .child(name_cell(name))
                                .when(is_active, |d| {
                                    let bg = if t.has("list.activeFileBackground") { t.bg("list.activeFileBackground") } else { t.bg("list.activeSelectionBackground") };
                                    let fg = if t.has("list.activeFileForeground") { t.c("list.activeFileForeground") } else { t.c("list.activeSelectionForeground") };
                                    d.bg(bg).text_color(fg)
                                })
                                .on_click(move |ev, w, cx| match (&toggle, is_dir) {
                                    (Some(toggle), true) => toggle(ev, w, cx),
                                    _ => e.update(cx, |_, cx| cx.emit(SolutionEvent::OpenFile(path.clone()))),
                                })
                                .into_any_element()
                        }
                        Entry::Reference { name, path, depth } => {
                            let e = entity.clone();
                            row(depth)
                                .tooltip(tooltip(path.display().to_string()))
                                .child(blank())
                                .child(icon("fileProject", px(12.), t.c("foreground.muted")))
                                .child(name_cell(name))
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

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::Path;

    use super::{nest_parent, project_item};

    #[test]
    fn nests_like_visual_studio() {
        let names = ["Index.cshtml", "Index.cshtml.cs", "Counter.razor", "Counter.razor.css", "appsettings.json", "appsettings.Development.json", "web.config", "web.Release.config", "Program.cs", "Program.Tests.cs", "Dockerfile", "Dockerfile.dev", ".env", ".env.local"];
        let files: HashSet<&str> = names.into_iter().collect();
        assert_eq!(nest_parent("Index.cshtml.cs", &files), Some("Index.cshtml"));
        assert_eq!(nest_parent("Counter.razor.css", &files), Some("Counter.razor"));
        assert_eq!(nest_parent("appsettings.Development.json", &files), Some("appsettings.json"));
        assert_eq!(nest_parent("web.Release.config", &files), Some("web.config"));
        // Not nested: unrelated .cs files, names without an extension, dotfiles.
        for name in ["Program.Tests.cs", "Program.cs", "Dockerfile.dev", ".env.local", "appsettings.json"] {
            assert_eq!(nest_parent(name, &files), None, "{name}");
        }
    }

    #[test]
    fn includes_what_an_sdk_project_includes() {
        let root = std::env::temp_dir().join(format!("bf-solution-items-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["bin", "obj", ".vs", "node_modules", "Pages", "Other"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(root.join("Other/Other.csproj"), "<Project />").unwrap();
        let item = |name: &str, is_dir: bool| project_item(&root.join(name), name, is_dir, true);
        assert!(item("Pages", true));
        assert!(item("Program.cs", false) && item(".editorconfig", false));
        for dir in ["bin", "obj", ".vs", "node_modules", "Other"] {
            assert!(!item(dir, true), "{dir}");
        }
        for file in ["App.csproj", "App.sln", "App.csproj.user"] {
            assert!(!item(file, false), "{file}");
        }
        // bin/obj are only special at the project root.
        assert!(project_item(Path::new("/nonexistent/Pages/bin"), "bin", true, false));
        let _ = std::fs::remove_dir_all(&root);
    }
}
