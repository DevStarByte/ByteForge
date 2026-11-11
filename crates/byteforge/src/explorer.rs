//! File explorer: a lazily expanded tree flattened into rows and rendered with a virtualized list.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use bf_core::Workspace;
use gpui::{
    App, ClickEvent, Context, EventEmitter, FocusHandle, Focusable, MouseButton, MouseDownEvent, Pixels, Point, UniformListScrollHandle, Window,
    div, prelude::*, px, uniform_list,
};

use crate::actions::RenameEntry;
use crate::assets::icon;
use crate::theme::Theme;

pub enum ExplorerEvent {
    /// Open a file; `true` moves the focus to the editor (double click), `false` keeps it in the explorer.
    Open(PathBuf, bool),
    /// Move a file or folder (dragged) into this folder.
    Move { from: PathBuf, to_dir: PathBuf },
    NewFile(PathBuf),
    NewFolder(PathBuf),
    /// New item from a template (class, interface, `dotnet new` item templates) in this folder.
    NewItem(PathBuf),
    Rename(PathBuf),
    Delete(PathBuf),
    OpenFolderRequested,
}

/// A file or folder being dragged; also draws itself under the mouse while dragging.
#[derive(Clone)]
pub struct DraggedEntry {
    path: PathBuf,
    name: String,
    is_dir: bool,
}

impl Render for DraggedEntry {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::global(cx);
        div()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .py(px(2.))
            .rounded(px(4.))
            .bg(t.bg("menu.background"))
            .text_color(t.c("menu.foreground"))
            .border_1()
            .border_color(t.c("menu.border"))
            .text_size(t.m("list.fontSize", 13.0))
            .child(file_icon_view(&self.name, self.is_dir, false, px(14.), &t))
            .child(self.name.clone())
    }
}

#[derive(Clone)]
struct Row {
    path: PathBuf,
    name: String,
    depth: usize,
    is_dir: bool,
}

pub struct Explorer {
    workspace: Workspace,
    expanded: HashSet<PathBuf>,
    rows: Vec<Row>,
    selected: Option<PathBuf>,
    /// The file in the active editor tab; it and its folders are highlighted.
    active: Option<PathBuf>,
    /// Automatically expand and scroll to the active file when it changes.
    pub auto_reveal: bool,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    menu: Option<(PathBuf, Point<Pixels>)>,
    /// Git decorations: letter and theme color key per path (folders get '•').
    decorations: HashMap<PathBuf, (char, &'static str)>,
}

impl EventEmitter<ExplorerEvent> for Explorer {}

impl Focusable for Explorer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Explorer {
    pub fn new(workspace: Workspace, cx: &mut Context<Self>) -> Self {
        let mut e = Self {
            workspace,
            expanded: HashSet::new(),
            rows: Vec::new(),
            selected: None,
            active: None,
            auto_reveal: true,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            menu: None,
            decorations: HashMap::new(),
        };
        e.refresh(cx);
        e
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.rows.clear();
        if let Some(root) = self.workspace.root() {
            self.push_children(&root, 0);
        }
        cx.notify();
    }

    fn push_children(&mut self, dir: &Path, depth: usize) {
        for (path, is_dir) in self.workspace.list_dir(dir) {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let expanded = is_dir && self.expanded.contains(&path);
            self.rows.push(Row { path: path.clone(), name, depth, is_dir });
            if expanded {
                self.push_children(&path, depth + 1);
            }
        }
    }

    /// Expands the parents of `path` and selects it.
    pub fn reveal(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.root() else { return };
        let mut dir = path.parent();
        while let Some(d) = dir {
            if !d.starts_with(&root) || d == root {
                break;
            }
            self.expanded.insert(d.to_path_buf());
            dir = d.parent();
        }
        self.selected = Some(path.to_path_buf());
        self.refresh(cx);
        if let Some(ix) = self.rows.iter().position(|r| r.path == path) {
            self.scroll.scroll_to_item(ix, gpui::ScrollStrategy::Center);
        }
    }

    /// Marks the file being edited. When it changes (and auto-reveal is on), its folders are expanded and
    /// it is scrolled into view.
    pub fn set_active(&mut self, path: Option<&Path>, cx: &mut Context<Self>) {
        if self.active.as_deref() == path {
            return;
        }
        self.active = path.map(Path::to_path_buf);
        match path {
            Some(path) if self.auto_reveal => self.reveal(path, cx),
            _ => cx.notify(),
        }
    }

    pub fn set_decorations(&mut self, decorations: HashMap<PathBuf, (char, &'static str)>, cx: &mut Context<Self>) {
        if decorations != self.decorations {
            self.decorations = decorations;
            cx.notify();
        }
    }

    /// Follows a rename or move of `from` to `to`: expanded folders, the selection and the active file below
    /// it keep their state under the new path.
    pub fn path_moved(&mut self, from: &Path, to: &Path, cx: &mut Context<Self>) {
        let remap = |p: &Path| p.strip_prefix(from).ok().map(|rest| if rest.as_os_str().is_empty() { to.to_path_buf() } else { to.join(rest) });
        self.expanded = self.expanded.iter().map(|p| remap(p).unwrap_or_else(|| p.clone())).collect();
        for p in [&mut self.selected, &mut self.active].into_iter().flatten() {
            if let Some(new) = remap(p) {
                *p = new;
            }
        }
        self.refresh(cx);
    }

    fn click(&mut self, ix: usize, focus_editor: bool, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(ix).cloned() else { return };
        self.selected = Some(row.path.clone());
        self.menu = None;
        if row.is_dir {
            if !self.expanded.remove(&row.path) {
                self.expanded.insert(row.path.clone());
            }
            self.refresh(cx);
        } else {
            cx.emit(ExplorerEvent::Open(row.path, focus_editor));
            cx.notify();
        }
    }

    fn target_dir(&self) -> Option<PathBuf> {
        match &self.selected {
            Some(p) if p.is_dir() => Some(p.clone()),
            Some(p) => p.parent().map(Path::to_path_buf),
            None => self.workspace.root(),
        }
    }
}

/// File icons declared by the turned-on modules, in module order: (rule, registered icon name).
static MODULE_FILE_ICONS: parking_lot::RwLock<Vec<(bf_core::plugins::FileIconRule, String)>> = parking_lot::RwLock::new(Vec::new());

/// Installs the file icons of these (turned-on) modules; they win over the built-in icons.
pub(crate) fn set_module_file_icons(modules: &[bf_core::plugins::ModuleManifest]) {
    let mut rules = Vec::new();
    let mut paths = std::collections::HashMap::new();
    for m in modules {
        for (i, rule) in m.file_icons.iter().enumerate() {
            let name = format!("module-{}-{i}", m.id);
            paths.insert(name.clone(), rule.icon.clone());
            rules.push((rule.clone(), name));
        }
    }
    *MODULE_FILE_ICONS.write() = rules;
    crate::assets::set_module_icons(paths);
}

/// Whether file icons come from macOS ("explorer.nativeIcons", on by default; macOS only).
static NATIVE_ICONS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(cfg!(target_os = "macos"));

pub(crate) fn set_native_icons(on: bool) {
    NATIVE_ICONS.store(on && cfg!(target_os = "macos"), std::sync::atomic::Ordering::Relaxed);
}

/// The icon for a file or folder: on macOS the one Finder shows (unless a module styles that file),
/// otherwise the theme's icon in its color.
pub(crate) fn file_icon_view(name: &str, is_dir: bool, expanded: bool, size: Pixels, t: &Theme) -> gpui::AnyElement {
    #[cfg(target_os = "macos")]
    if NATIVE_ICONS.load(std::sync::atomic::Ordering::Relaxed) && (is_dir || !MODULE_FILE_ICONS.read().iter().any(|(r, _)| r.matches(name))) {
        // Rendered at 2× so it stays sharp on Retina displays.
        let pixels = (f32::from(size) * 2.0).ceil() as u32;
        if let Some(image) = crate::native_icons::icon(&crate::native_icons::Kind::of(name, is_dir), pixels) {
            return gpui::img(image).size(size).flex_none().into_any_element();
        }
    }
    let (icon_name, color_key) = file_icon(name, is_dir, expanded);
    icon(&icon_name, size, file_icon_color(t, &color_key)).into_any_element()
}

/// The icon and its color for a file or folder name: (icon name, theme color key or color).
pub(crate) fn file_icon(name: &str, is_dir: bool, expanded: bool) -> (std::borrow::Cow<'static, str>, std::borrow::Cow<'static, str>) {
    if is_dir {
        return ((if expanded { "folderOpen" } else { "folder" }).into(), "icon.folder".into());
    }
    if let Some((rule, icon_name)) = MODULE_FILE_ICONS.read().iter().find(|(r, _)| r.matches(name)) {
        return (icon_name.clone().into(), rule.color.clone().unwrap_or_else(|| "icon.file".to_owned()).into());
    }
    let (icon_name, color) = builtin_file_icon(name);
    (icon_name.into(), color.into())
}

/// The built-in icon for a file name: (icon name, theme color key).
fn builtin_file_icon(name: &str) -> (&'static str, &'static str) {
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        ".gitignore" | ".gitattributes" | ".gitmodules" | ".editorconfig" | ".env" | "global.json" | "nuget.config" => {
            return ("fileConfig", "icon.config");
        }
        "license" | "licence" | "copying" => return ("fileText", "icon.text"),
        "makefile" | "justfile" => return ("fileShell", "icon.shell"),
        _ => {}
    }
    match Path::new(&lower).extension().and_then(|e| e.to_str()).unwrap_or("") {
        "cs" | "csx" => ("fileCsharp", "icon.csharp"),
        "csproj" | "fsproj" | "vbproj" | "props" | "targets" => ("fileProject", "icon.project"),
        "sln" | "slnx" | "slnf" => ("fileSolution", "icon.solution"),
        "json" | "jsonc" => ("fileJson", "icon.json"),
        "razor" | "cshtml" => ("fileRazor", "icon.razor"),
        "xml" | "axaml" | "xaml" | "resx" | "html" | "htm" | "config" | "manifest" | "nuspec" => ("fileMarkup", "icon.markup"),
        "md" | "markdown" => ("fileMarkdown", "icon.markdown"),
        "yml" | "yaml" => ("fileYaml", "icon.yaml"),
        "toml" | "ini" | "env" | "editorconfig" | "conf" | "cfg" | "runsettings" | "ruleset" => ("fileConfig", "icon.config"),
        "txt" | "log" | "csv" | "tsv" => ("fileText", "icon.text"),
        "sh" | "bash" | "zsh" | "fish" | "ps1" | "psm1" | "psd1" | "bat" | "cmd" => ("fileShell", "icon.shell"),
        "js" | "mjs" | "cjs" | "jsx" | "ts" | "mts" | "cts" | "tsx" => ("fileScript", "icon.script"),
        "css" | "scss" | "sass" | "less" => ("fileStyle", "icon.style"),
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "ico" | "webp" | "svg" | "avif" => ("fileImage", "icon.image"),
        "zip" | "nupkg" | "snupkg" | "gz" | "tgz" | "tar" | "7z" | "rar" | "xz" => ("fileArchive", "icon.archive"),
        "dll" | "exe" | "pdb" | "so" | "dylib" | "bin" | "wasm" => ("fileBinary", "icon.binary"),
        _ => ("file", "icon.file"),
    }
}

/// Theme color for a file icon key (or a literal color from a module). Themes written before a key existed
/// fall back to a related one.
pub(crate) fn file_icon_color(t: &Theme, key: &str) -> gpui::Hsla {
    if let Some(color) = key.starts_with('#').then(|| crate::theme::parse_color(key)).flatten() {
        return color;
    }
    let mut key = key;
    while !t.has(key) {
        key = match key {
            "icon.solution" => "icon.project",
            "icon.razor" | "icon.yaml" => "icon.markup",
            "icon.script" | "icon.shell" => "icon.json",
            "icon.style" | "icon.image" => "icon.markdown",
            "icon.archive" => "icon.folder",
            "icon.config" | "icon.text" | "icon.binary" => "icon.file",
            _ => return t.c("icon.file"),
        };
    }
    t.c(key)
}

impl Render for Explorer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::global(cx);
        let Some(root) = self.workspace.root() else {
            return div()
                .p_4()
                .flex()
                .flex_col()
                .gap_3()
                .child("You have not yet opened a folder.")
                .child(
                    div()
                        .id("open-folder")
                        .px_3()
                        .py_1()
                        .rounded(t.radius("button.radius", 4.0))
                        .bg(t.bg("button.background"))
                        .text_color(t.c("button.foreground"))
                        .cursor_pointer()
                        .flex()
                        .justify_center()
                        .hover(|s| s.bg(t.c("button.hoverBackground")))
                        .child("Open Folder")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(ExplorerEvent::OpenFolderRequested))),
                )
                .into_any_element();
        };
        let row_h = t.n("list.itemHeight", 22.0);
        let icon_size = t.m("explorer.iconSize", 15.0);
        let spacing = t.m("explorer.itemSpacing", 6.0);
        let entity = cx.entity();
        let selected = self.selected.clone();
        let active = self.active.clone();
        let count = self.rows.len();
        let indicator = if t.has("list.activeFileIndicator") {
            t.c("list.activeFileIndicator")
        } else if t.has("focusBorder") {
            t.c("focusBorder")
        } else {
            t.c_or("tab.activeBorderTop", 0x0078d4ff)
        };
        let indicator_w = t.n("list.activeFileIndicatorWidth", 2.0);
        let header_name = root.file_name().map(|n| n.to_string_lossy().to_uppercase()).unwrap_or_default();
        let header_button = |id: &'static str, name: &'static str, title: &'static str| {
            div().id(id).p(px(3.)).rounded(px(4.)).cursor_pointer().hover(|s| s.bg(t.c("toolbar.hoverBackground"))).tooltip(crate::workbench::tooltip(title)).child(icon(name, px(14.), t.c("icon.foreground")))
        };

        let list = uniform_list("explorer", count, {
            let entity = entity.clone();
            move |range, _, cx| {
                let explorer = entity.read(cx);
                let t = Theme::global(cx);
                range
                    .map(|ix| {
                        let row = &explorer.rows[ix];
                        let is_active = active.as_ref() == Some(&row.path);
                        // A folder that holds the active file (the trail to it, also visible when collapsed).
                        let holds_active = row.is_dir && active.as_ref().is_some_and(|a| a.starts_with(&row.path));
                        let is_selected = selected.as_ref() == Some(&row.path) && !is_active;
                        let expanded = row.is_dir && explorer.expanded.contains(&row.path);
                        let deco = explorer.decorations.get(&row.path).copied();
                        let e1 = entity.clone();
                        let e2 = entity.clone();
                        let path = row.path.clone();
                        let drop_dir = if row.is_dir { Some(row.path.clone()) } else { row.path.parent().map(Path::to_path_buf) };
                        let dragged = DraggedEntry { path: row.path.clone(), name: row.name.clone(), is_dir: row.is_dir };
                        let e3 = entity.clone();
                        let drop_highlight = t.c("list.hoverBackground");
                        div()
                            .id(ix)
                            .relative()
                            .w_full()
                            .h(px(row_h))
                            .pl(px(8.0 + row.depth as f32 * 12.0))
                            .pr_2()
                            .flex()
                            .items_center()
                            .gap(spacing)
                            .rounded(t.radius("list.radius", 0.0))
                            .cursor_pointer()
                            .text_size(t.m("list.fontSize", 13.0))
                            .when(is_active, |d| {
                                let bg = if t.has("list.activeFileBackground") { t.bg("list.activeFileBackground") } else { t.bg("list.activeSelectionBackground") };
                                let fg = if t.has("list.activeFileForeground") { t.c("list.activeFileForeground") } else { t.c("list.activeSelectionForeground") };
                                d.bg(bg)
                                    .text_color(fg)
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(div().absolute().left_0().top_0().bottom_0().w(px(indicator_w)).bg(indicator))
                            })
                            .when(is_selected, |d| d.bg(t.c("list.hoverBackground")))
                            .when(!is_active && !is_selected, |d| d.hover(|s| s.bg(t.c("list.hoverBackground"))))
                            .when(holds_active, |d| d.font_weight(gpui::FontWeight::SEMIBOLD))
                            .child(div().w(px(12.)).when(row.is_dir, |d| {
                                d.child(icon(if expanded { "chevronDown" } else { "chevronRight" }, px(12.), t.c("icon.foreground")))
                            }))
                            .child(file_icon_view(&row.name, row.is_dir, expanded, icon_size, &t))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .when_some(deco, |d, (_, key)| d.text_color(t.c(key)))
                                    .child(row.name.clone()),
                            )
                            .when_some(deco, |d, (letter, key)| d.child(div().flex_none().text_size(px(11.)).text_color(t.c(key)).child(letter.to_string())))
                            .on_click(move |ev: &ClickEvent, window, cx| {
                                e1.update(cx, |e, cx| {
                                    window.focus(&e.focus);
                                    e.click(ix, ev.click_count() > 1, cx)
                                })
                            })
                            .on_drag(dragged, |d, _, _, cx| cx.new(|_| d.clone()))
                            .when(row.is_dir, |d| d.drag_over::<DraggedEntry>(move |s, _, _, _| s.bg(drop_highlight)))
                            .on_drop(move |d: &DraggedEntry, _, cx| {
                                if let Some(to_dir) = drop_dir.clone() {
                                    e3.update(cx, |_, cx| cx.emit(ExplorerEvent::Move { from: d.path.clone(), to_dir }));
                                }
                            })
                            .on_mouse_down(MouseButton::Right, move |ev: &MouseDownEvent, window, cx| {
                                e2.update(cx, |e, cx| {
                                    window.focus(&e.focus);
                                    e.selected = Some(path.clone());
                                    e.menu = Some((path.clone(), ev.position));
                                    cx.notify();
                                })
                            })
                    })
                    .collect()
            }
        })
        .track_scroll(self.scroll.clone())
        .flex_1();

        let menu = self.menu.clone().map(|(path, pos)| {
            let item = |id: &'static str, label: &'static str| crate::menu::item(&t, id, label, None);
            let (p1, p2, p3, p4, p5, p6) = (path.clone(), path.clone(), path.clone(), path.clone(), path.clone(), path.clone());
            gpui::deferred(
                gpui::anchored().position(pos).child(
                    crate::menu::panel(&t, 180.)
                        .on_mouse_down_out(cx.listener(|e, _, _, cx| {
                            e.menu = None;
                            cx.notify();
                        }))
                        .child(item("m-newfile", "New File…").on_click(cx.listener(move |e, _, _, cx| {
                            e.menu = None;
                            let dir = if p1.is_dir() { p1.clone() } else { p1.parent().map(Path::to_path_buf).unwrap_or_default() };
                            cx.emit(ExplorerEvent::NewFile(dir));
                        })))
                        .child(item("m-newitem", "New Item from Template…").on_click(cx.listener(move |e, _, _, cx| {
                            e.menu = None;
                            let dir = if p6.is_dir() { p6.clone() } else { p6.parent().map(Path::to_path_buf).unwrap_or_default() };
                            cx.emit(ExplorerEvent::NewItem(dir));
                        })))
                        .child(item("m-newfolder", "New Folder…").on_click(cx.listener(move |e, _, _, cx| {
                            e.menu = None;
                            let dir = if p2.is_dir() { p2.clone() } else { p2.parent().map(Path::to_path_buf).unwrap_or_default() };
                            cx.emit(ExplorerEvent::NewFolder(dir));
                        })))
                        .child(item("m-rename", "Rename…").on_click(cx.listener(move |e, _, _, cx| {
                            e.menu = None;
                            cx.emit(ExplorerEvent::Rename(p3.clone()));
                        })))
                        .child(crate::menu::separator(&t))
                        .child(item("m-delete", "Delete").on_click(cx.listener(move |e, _, _, cx| {
                            e.menu = None;
                            cx.emit(ExplorerEvent::Delete(p4.clone()));
                        })))
                        .child(crate::menu::separator(&t))
                        .child(item("m-copy", "Copy Path").on_click(cx.listener(move |e, _, _, cx| {
                            e.menu = None;
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(p5.display().to_string()));
                            cx.notify();
                        })))
                        .child(item("m-reveal", "Reveal in File Manager").on_click(cx.listener(move |e, _, _, cx| {
                            e.menu = None;
                            cx.reveal_path(&path);
                            cx.notify();
                        }))),
                ),
            )
            .with_priority(1)
        });

        div()
            .track_focus(&self.focus)
            .key_context("Explorer")
            .on_action(cx.listener(|e, _: &RenameEntry, _, cx| {
                if let Some(path) = e.selected.clone() {
                    cx.emit(ExplorerEvent::Rename(path));
                }
            }))
            // Dropped outside any row (empty space, header): move to the workspace root.
            .on_drop(cx.listener(move |_, d: &DraggedEntry, _, cx| cx.emit(ExplorerEvent::Move { from: d.path.clone(), to_dir: root.clone() })))
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .px_3()
                    .py_1()
                    .child(
                        div()
                            .flex_1()
                            .text_size(t.m("sideBarSectionHeader.fontSize", 11.0))
                            .font_weight(t.weight("sideBarSectionHeader.weight", gpui::FontWeight::BOLD))
                            .text_color(t.c("sideBarSectionHeader.foreground"))
                            .child(header_name),
                    )
                    .child(header_button("x-newfile", "file", "New File").on_click(cx.listener(|e, _, _, cx| {
                        if let Some(dir) = e.target_dir() {
                            cx.emit(ExplorerEvent::NewFile(dir));
                        }
                    })))
                    .child(header_button("x-newfolder", "folder", "New Folder").on_click(cx.listener(|e, _, _, cx| {
                        if let Some(dir) = e.target_dir() {
                            cx.emit(ExplorerEvent::NewFolder(dir));
                        }
                    })))
                    .child(header_button("x-collapse", "chevronUp", "Collapse All").on_click(cx.listener(|e, _, _, cx| {
                        e.expanded.clear();
                        e.refresh(cx);
                    }))),
            )
            .child(list)
            .children(menu)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{builtin_file_icon, file_icon, set_module_file_icons};

    #[test]
    fn picks_an_icon_per_file_type() {
        assert_eq!(builtin_file_icon("Program.cs"), ("fileCsharp", "icon.csharp"));
        assert_eq!(builtin_file_icon("App.csproj"), ("fileProject", "icon.project"));
        assert_eq!(builtin_file_icon("App.sln"), ("fileSolution", "icon.solution"));
        assert_eq!(builtin_file_icon("Index.cshtml"), ("fileRazor", "icon.razor"));
        assert_eq!(builtin_file_icon("appsettings.Development.json"), ("fileJson", "icon.json"));
        assert_eq!(builtin_file_icon("logo.SVG"), ("fileImage", "icon.image"));
        assert_eq!(builtin_file_icon(".gitignore"), ("fileConfig", "icon.config"));
        assert_eq!(builtin_file_icon("notes"), ("file", "icon.file"));
        assert_eq!(file_icon("src", true, true), ("folderOpen".into(), "icon.folder".into()));
        assert_eq!(builtin_file_icon("Dockerfile"), ("file", "icon.file"));
    }

    #[test]
    fn module_icons_win_while_installed() {
        let module: bf_core::plugins::ModuleManifest = serde_json::from_str(
            r#"{ "id": "t.docker", "name": "Docker", "fileIcons": [{ "icon": "M0,0Z", "color": "icon.docker", "names": ["Dockerfile"] }] }"#,
        )
        .unwrap();
        set_module_file_icons(std::slice::from_ref(&module));
        assert_eq!(file_icon("Dockerfile", false, false), ("module-t.docker-0".into(), "icon.docker".into()));
        set_module_file_icons(&[]);
        assert_eq!(file_icon("Dockerfile", false, false), ("file".into(), "icon.file".into()));
    }
}
