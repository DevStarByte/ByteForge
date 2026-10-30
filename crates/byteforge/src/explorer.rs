//! File explorer: a lazily expanded tree flattened into rows and rendered with a virtualized list.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use bf_core::Workspace;
use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, MouseButton, MouseDownEvent, Pixels, Point, UniformListScrollHandle, Window, div,
    prelude::*, px, uniform_list,
};

use crate::assets::icon;
use crate::theme::Theme;

pub enum ExplorerEvent {
    Open(PathBuf),
    NewFile(PathBuf),
    NewFolder(PathBuf),
    Rename(PathBuf),
    Delete(PathBuf),
    OpenFolderRequested,
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

    pub fn set_decorations(&mut self, decorations: HashMap<PathBuf, (char, &'static str)>, cx: &mut Context<Self>) {
        if decorations != self.decorations {
            self.decorations = decorations;
            cx.notify();
        }
    }

    fn click(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(ix).cloned() else { return };
        self.selected = Some(row.path.clone());
        self.menu = None;
        if row.is_dir {
            if !self.expanded.remove(&row.path) {
                self.expanded.insert(row.path.clone());
            }
            self.refresh(cx);
        } else {
            cx.emit(ExplorerEvent::Open(row.path));
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

pub(crate) fn icon_color_key(name: &str, is_dir: bool) -> &'static str {
    if is_dir {
        return "icon.folder";
    }
    match Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase().as_str() {
        "cs" | "csx" => "icon.csharp",
        "csproj" | "sln" | "slnx" | "props" | "targets" | "toml" => "icon.project",
        "json" => "icon.json",
        "xml" | "axaml" | "xaml" | "config" | "resx" => "icon.markup",
        "md" => "icon.markdown",
        _ => "icon.file",
    }
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
        let count = self.rows.len();
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
                        let is_selected = selected.as_ref() == Some(&row.path);
                        let expanded = row.is_dir && explorer.expanded.contains(&row.path);
                        let deco = explorer.decorations.get(&row.path).copied();
                        let e1 = entity.clone();
                        let e2 = entity.clone();
                        let path = row.path.clone();
                        div()
                            .id(ix)
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
                            .when(is_selected, |d| d.bg(t.bg("list.activeSelectionBackground")).text_color(t.c("list.activeSelectionForeground")))
                            .when(!is_selected, |d| d.hover(|s| s.bg(t.c("list.hoverBackground"))))
                            .child(div().w(px(12.)).when(row.is_dir, |d| {
                                d.child(icon(if expanded { "chevronDown" } else { "chevronRight" }, px(12.), t.c("icon.foreground")))
                            }))
                            .child(icon(if row.is_dir { if expanded { "folderOpen" } else { "folder" } } else { "file" }, icon_size, t.c(icon_color_key(&row.name, row.is_dir))))
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
                            .on_click(move |_, _, cx| e1.update(cx, |e, cx| e.click(ix, cx)))
                            .on_mouse_down(MouseButton::Right, move |ev: &MouseDownEvent, _, cx| {
                                e2.update(cx, |e, cx| {
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
            let item = |id: &'static str, label: &'static str| {
                div().id(id).px_3().py_1().rounded(px(4.)).cursor_pointer().hover(|s| s.bg(t.c("menu.selectionBackground"))).child(label)
            };
            let (p1, p2, p3, p4, p5) = (path.clone(), path.clone(), path.clone(), path.clone(), path.clone());
            gpui::deferred(
                gpui::anchored().position(pos).child(
                    div()
                        .min_w(px(180.))
                        .bg(t.bg("menu.background"))
                        .text_color(t.c("menu.foreground"))
                        .border_1()
                        .border_color(t.c("menu.border"))
                        .rounded(t.radius("widget.radius", 6.0))
                        .shadow(t.shadow("widget.shadow"))
                        .p_1()
                        .text_size(t.m("ui.size", 13.0))
                        .on_mouse_down_out(cx.listener(|e, _, _, cx| {
                            e.menu = None;
                            cx.notify();
                        }))
                        .child(item("m-newfile", "New File…").on_click(cx.listener(move |e, _, _, cx| {
                            e.menu = None;
                            let dir = if p1.is_dir() { p1.clone() } else { p1.parent().map(Path::to_path_buf).unwrap_or_default() };
                            cx.emit(ExplorerEvent::NewFile(dir));
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
                        .child(item("m-delete", "Delete").on_click(cx.listener(move |e, _, _, cx| {
                            e.menu = None;
                            cx.emit(ExplorerEvent::Delete(p4.clone()));
                        })))
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
