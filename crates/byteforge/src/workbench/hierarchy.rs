//! The Hierarchy panel: Show Call Hierarchy (callers / callees) and Show Type Hierarchy (base types /
//! derived types) as a tree whose branches are asked for from the language server when opened.

use super::*;

pub(super) struct Hierarchy {
    /// A call hierarchy (else a type hierarchy).
    pub calls: bool,
    /// Callers (else callees) for calls, base types (else derived types) for types.
    pub up: bool,
    /// The tree, flattened: node 0 is the symbol it was opened on.
    pub nodes: Vec<HierarchyNode>,
    /// Bumped on every new tree, so answers for an older one are dropped.
    generation: u64,
}

pub(super) struct HierarchyNode {
    pub item: bf_lsp::HierarchyItem,
    pub depth: usize,
    /// `None` until asked for.
    pub children: Option<Vec<usize>>,
    pub expanded: bool,
    /// Where the calls are (incoming: in this node's file; outgoing: in its parent's), first one first.
    pub sites: Vec<bf_lsp::Range>,
    pub sites_path: Option<PathBuf>,
}

impl Hierarchy {
    pub fn root(&self) -> &bf_lsp::HierarchyItem {
        &self.nodes[0].item
    }

    /// What the tree shows, e.g. "callers of 'Add'".
    pub fn title(&self) -> String {
        let what = match (self.calls, self.up) {
            (true, true) => "callers of",
            (true, false) => "calls from",
            (false, true) => "base types of",
            (false, false) => "derived types of",
        };
        format!("{what} '{}'", short_name(&self.root().name))
    }

    /// The nodes in display order (children under their expanded parent).
    pub fn visible(&self) -> Vec<usize> {
        fn walk(h: &Hierarchy, ix: usize, out: &mut Vec<usize>) {
            out.push(ix);
            let node = &h.nodes[ix];
            if node.expanded {
                for &child in node.children.iter().flatten() {
                    walk(h, child, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(self, 0, &mut out);
        out
    }
}

/// `Add(int, int)` → `Add`.
fn short_name(name: &str) -> &str {
    name.split(['(', '<']).next().unwrap_or(name).trim()
}

/// A child node to add: the item, its call sites and the file they're in.
type Child = (bf_lsp::HierarchyItem, Vec<bf_lsp::Range>, Option<PathBuf>);

static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl Workbench {
    /// Opens the Hierarchy panel on `item`: callers for calls, derived types for types (the direction can be switched).
    pub(super) fn open_hierarchy(&mut self, calls: bool, item: bf_lsp::HierarchyItem, cx: &mut Context<Self>) {
        let up = calls;
        self.set_hierarchy(calls, up, item, cx);
        self.show_panel(PanelView::Hierarchy, cx);
    }

    fn set_hierarchy(&mut self, calls: bool, up: bool, item: bf_lsp::HierarchyItem, cx: &mut Context<Self>) {
        let root = HierarchyNode { item, depth: 0, children: None, expanded: false, sites: Vec::new(), sites_path: None };
        let generation = GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.hierarchy = Some(Hierarchy { calls, up, nodes: vec![root], generation });
        self.toggle_hierarchy_node(0, cx);
    }

    /// Switches between callers and callees (or base and derived types), starting again from the same symbol.
    pub(super) fn set_hierarchy_direction(&mut self, up: bool, cx: &mut Context<Self>) {
        let Some(h) = &self.hierarchy else { return };
        if h.up == up {
            return;
        }
        let (calls, item) = (h.calls, h.root().clone());
        self.set_hierarchy(calls, up, item, cx);
    }

    /// Opens or closes a node; opening one the first time asks the server for its children.
    pub(super) fn toggle_hierarchy_node(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(h) = &mut self.hierarchy else { return };
        let Some(node) = h.nodes.get_mut(ix) else { return };
        node.expanded = !node.expanded;
        cx.notify();
        if !node.expanded || node.children.is_some() {
            return;
        }
        let Some(lsp) = self.lsp.clone() else {
            self.log("The hierarchy needs the C# language server, which isn't running.");
            return;
        };
        let (calls, up, generation, item) = (h.calls, h.up, h.generation, node.item.clone());
        let parent_path = item.path.clone();
        cx.spawn(async move |this, cx| {
            let children: anyhow::Result<Vec<Child>> = if calls {
                match lsp.hierarchy_calls(&item, up).recv_async().await {
                    Ok(r) => r.map(|calls| {
                        calls
                            .into_iter()
                            .map(|c| {
                                let sites_path = Some(if up { c.item.path.clone() } else { parent_path.clone() });
                                (c.item, c.ranges, sites_path)
                            })
                            .collect()
                    }),
                    Err(_) => return,
                }
            } else {
                match lsp.hierarchy_types(&item, up).recv_async().await {
                    Ok(r) => r.map(|types| types.into_iter().map(|t| (t, Vec::new(), None)).collect()),
                    Err(_) => return,
                }
            };
            let _ = this.update(cx, |wb, cx| {
                let children = match children {
                    Ok(children) => children,
                    Err(e) => {
                        wb.log(format!("The hierarchy of '{}' couldn't be read: {e:#}", short_name(&item.name)));
                        Vec::new()
                    }
                };
                let Some(h) = &mut wb.hierarchy else { return };
                if h.generation != generation {
                    return;
                }
                let depth = h.nodes[ix].depth + 1;
                let mut ids = Vec::new();
                for (item, sites, sites_path) in children {
                    ids.push(h.nodes.len());
                    h.nodes.push(HierarchyNode { item, depth, children: None, expanded: false, sites, sites_path });
                }
                h.nodes[ix].children = Some(ids);
                cx.notify();
            });
        })
        .detach();
    }

    /// Opens a node: a caller at its (first) call, anything else at its declaration.
    pub(super) fn open_hierarchy_node(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(h) = &self.hierarchy else { return };
        let Some(node) = h.nodes.get(ix) else { return };
        let (path, position) = match (&node.sites_path, node.sites.first()) {
            (Some(path), Some(site)) if h.up => (path.clone(), site.start),
            _ => (node.item.path.clone(), node.item.selection_range.start),
        };
        let path = normalize_path(&path);
        let at = self.lsp_point(&path, position, cx);
        self.open_file(path, Some(at), window, cx);
    }

    /// An LSP position (UTF-16 columns) as a point, from the open editor or the file on disk.
    fn lsp_point(&self, path: &Path, position: bf_lsp::Position, cx: &App) -> Point {
        let (line, col) = (position.line as usize, position.character as usize);
        if let Some(tab) = self.tabs.iter().find(|t| t.path.as_deref().is_some_and(|p| bf_core::paths::same_path(p, path))) {
            return tab.editor.read(cx).state.buffer.utf16_to_point(line, col);
        }
        match std::fs::read_to_string(path) {
            Ok(text) => bf_text::Buffer::new(&text).utf16_to_point(line, col),
            Err(_) => Point::new(line, col),
        }
    }

    pub(super) fn render_hierarchy(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = Theme::global(cx);
        let muted = t.c("foreground.muted");
        let Some(h) = &self.hierarchy else {
            let call = crate::actions::key_label("shift-alt-h");
            return div()
                .p_3()
                .text_color(muted)
                .child(format!("Use Show Call Hierarchy ({call}) on a method or Show Type Hierarchy on a type (Go menu, or right-click)."))
                .into_any_element();
        };
        let root = self.workspace.root();
        let (up_label, down_label) = if h.calls { ("Callers", "Callees") } else { ("Base Types", "Derived Types") };
        let toggle = |id: &'static str, label: &'static str, active: bool| {
            div()
                .id(id)
                .px_2()
                .rounded(px(3.))
                .cursor_pointer()
                .text_color(if active { t.c("foreground") } else { muted })
                .when(active, |d| d.bg(t.c("list.activeSelectionBackground")))
                .hover(|s| s.bg(t.c("list.hoverBackground")))
                .child(label)
        };
        let mut rows: Vec<gpui::AnyElement> = vec![
            div()
                .px_3()
                .py(px(2.))
                .flex()
                .items_center()
                .gap_2()
                .child(div().whitespace_nowrap().text_color(muted).child(h.title()))
                .child(toggle("hier-up", up_label, h.up).on_click(cx.listener(|wb, _, _, cx| wb.set_hierarchy_direction(true, cx))))
                .child(toggle("hier-down", down_label, !h.up).on_click(cx.listener(|wb, _, _, cx| wb.set_hierarchy_direction(false, cx))))
                .into_any_element(),
        ];
        for ix in h.visible() {
            let node = &h.nodes[ix];
            let leaf = node.children.as_ref().is_some_and(Vec::is_empty);
            let loading = node.expanded && node.children.is_none();
            let file = node.item.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let dir = node.item.path.parent().map(|p| root.as_ref().and_then(|r| p.strip_prefix(r).ok()).unwrap_or(p).display().to_string()).unwrap_or_default();
            let mut place = format!("{file}:{}", node.item.selection_range.start.line + 1);
            if !dir.is_empty() {
                place = format!("{place}  {dir}");
            }
            let calls = match node.sites.len() {
                0 | 1 => String::new(),
                n => format!("{n} calls"),
            };
            let chevron = div()
                .id(SharedString::from(format!("hier-chevron-{ix}")))
                .w(px(16.))
                .flex_none()
                .flex()
                .justify_center()
                .when(!leaf, |d| {
                    d.cursor_pointer()
                        .child(icon(if node.expanded { "chevronDown" } else { "chevronRight" }, px(14.), t.c("icon.foreground")))
                        .on_click(cx.listener(move |wb, _, _, cx| {
                            cx.stop_propagation();
                            wb.toggle_hierarchy_node(ix, cx);
                        }))
                });
            rows.push(
                div()
                    .id(SharedString::from(format!("hier-{ix}")))
                    .pl(px(12. + 16. * node.depth as f32))
                    .pr_3()
                    .py(px(1.))
                    .flex()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .hover(|s| s.bg(t.c("list.hoverBackground")))
                    .child(chevron)
                    .child(div().w(px(64.)).flex_none().text_size(px(11.)).text_color(muted).child(node.item.kind))
                    .child(div().whitespace_nowrap().font_family(t.editor_font.clone()).child(node.item.name.clone()))
                    .when_some(node.item.detail.clone(), |d, detail| d.child(div().whitespace_nowrap().text_size(px(11.)).text_color(muted).child(detail)))
                    .when(!calls.is_empty(), |d| {
                        d.child(div().px(px(6.)).rounded(px(9.)).text_size(px(11.)).bg(t.bg("badge.background")).text_color(t.c("badge.foreground")).child(calls))
                    })
                    .child(div().whitespace_nowrap().text_size(px(11.)).text_color(muted).child(place))
                    .when(loading, |d| d.child(div().text_size(px(11.)).text_color(muted).child("…")))
                    .on_click(cx.listener(move |wb, e: &gpui::ClickEvent, w, cx| {
                        // A click opens the symbol; a double click also opens or closes its branch.
                        if e.click_count() >= 2 {
                            wb.toggle_hierarchy_node(ix, cx);
                        } else {
                            wb.open_hierarchy_node(ix, w, cx);
                        }
                    }))
                    .into_any_element(),
            );
            if node.expanded && leaf {
                let none = match (h.calls, h.up) {
                    (true, true) => "no callers in the solution",
                    (true, false) => "no calls into the solution",
                    (false, true) => "no base types in the solution",
                    (false, false) => "no derived types",
                };
                rows.push(div().pl(px(12. + 16. * (node.depth + 1) as f32 + 20.)).text_size(px(11.)).text_color(muted).child(none).into_any_element());
            }
        }
        div().id("hierarchy").size_full().overflow_y_scroll().text_size(px(12.)).children(rows).into_any_element()
    }
}
