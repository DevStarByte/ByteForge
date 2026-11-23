//! The editor area: tab strips, breadcrumbs, pages and the welcome screen.

use super::*;

/// A tab being dragged to another place or group; draws its title under the mouse.
#[derive(Clone)]
pub(super) struct DraggedTab {
    pub index: usize,
    title: String,
}

impl Render for DraggedTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::global(cx);
        div()
            .px_3()
            .py_1()
            .rounded(px(4.))
            .bg(t.bg("tab.activeBackground"))
            .border_1()
            .border_color(t.c("menu.border"))
            .text_color(t.c("tab.activeForeground"))
            .text_size(t.m("tab.fontSize", 13.0))
            .shadow(t.shadow("widget.shadow"))
            .child(self.title.clone())
    }
}

impl Workbench {
    /// The editor column; `editor.order` arranges tabs, breadcrumbs, content and the panel.
    pub(super) fn render_main(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::global(cx);
        let lay = &t.layout;
        let split = self.is_split();
        let tabs_at_bottom = lay.tabs_below_content();
        let show_tabs = lay.editor.iter().any(|p| p == "tabs");
        let show_crumbs = lay.editor.iter().any(|p| p == "breadcrumbs");
        let drop_color = t.c("focusBorder");
        let mut content: Option<gpui::AnyElement> = Some(if split {
            // Two groups side by side, each with its own tab strip.
            let border = t.c_or("editorGroup.border", 0x80808040);
            let groups: Vec<_> = (0..2)
                .map(|g| {
                    let shown = self.visible[g].and_then(|i| self.tabs.get(i)).map(|t| (t.editor.clone(), t.page.clone()));
                    let editor: Option<gpui::AnyElement> = shown.map(|(editor, page)| self.render_page(page, editor, cx));
                    let tabs = show_tabs.then(|| self.render_tabs(tabs_at_bottom, g, cx));
                    // Each group has its own breadcrumbs, of the file it shows, right above its editor.
                    let crumbs = if show_crumbs { self.render_breadcrumbs(self.visible[g], g, cx).map(|c| c.into_any_element()) } else { None };
                    // A tab dropped on a group's editor goes to the end of that group.
                    let body = div()
                        .id(SharedString::from(format!("editor-group-body-{g}")))
                        .debug_selector(|| format!("editor-group-body-{g}"))
                        .flex_1()
                        .min_h_0()
                        .children(editor)
                        .drag_over::<DraggedTab>(move |s, _, _, _| s.border_2().border_color(drop_color))
                        .on_drop(cx.listener(move |wb, d: &DraggedTab, w, cx| wb.move_tab(d.index, g, None, w, cx)));
                    div()
                        .id(SharedString::from(format!("editor-group-{g}")))
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .flex_col()
                        .when(g == 1, |d| d.border_l_1().border_color(border))
                        .map(|d| {
                            let tabs = tabs.map(|t| t.into_any_element());
                            if tabs_at_bottom { d.children(crumbs).child(body).children(tabs) } else { d.children(tabs).children(crumbs).child(body) }
                        })
                        .on_mouse_down(MouseButton::Left, cx.listener(move |wb, _, w, cx| {
                            if wb.focus_group != g {
                                wb.focus_group(g, w, cx);
                            }
                        }))
                        .into_any_element()
                })
                .collect();
            div().size_full().flex().children(groups).into_any_element()
        } else {
            match self.active.and_then(|i| self.tabs.get(i)).map(|t| (t.editor.clone(), t.page.clone())) {
                Some((editor, page)) => self.render_page(page, editor, cx),
                None => self.render_welcome(window, cx).into_any_element(),
            }
        });
        let mut col = div().flex_1().min_w_0().h_full().flex().flex_col();
        for part in &lay.editor {
            match part.as_str() {
                "tabs" if !self.tabs.is_empty() && !split => col = col.child(self.render_tabs(tabs_at_bottom, 0, cx)),
                "breadcrumbs" if !split => col = col.children(self.render_breadcrumbs(self.active, 0, cx)),
                "content" => {
                    if let Some(c) = content.take() {
                        col = col.child(self.content_drop_zone(split, c, cx));
                    }
                }
                "panel" if self.panel_visible && !lay.panel_in_main() => {
                    let place = if lay.panel_above_content() { PanelPlace::Top } else { PanelPlace::Bottom };
                    col = col.child(self.render_panel(place, cx));
                }
                _ => {}
            }
        }
        if let Some(c) = content {
            col = col.child(self.content_drop_zone(split, c, cx));
        }
        col
    }

    /// The editor content; with one group, a tab dropped on it opens a second group on the right.
    fn content_drop_zone(&self, split: bool, content: gpui::AnyElement, cx: &mut Context<Self>) -> gpui::AnyElement {
        let zone = div().id("editor-content").debug_selector(|| "editor-content".into()).flex_1().min_h_0().child(content);
        if split {
            return zone.into_any_element();
        }
        let drop_color = Theme::global(cx).c("focusBorder");
        zone.drag_over::<DraggedTab>(move |s, _, _, _| s.border_r_4().border_color(drop_color))
            .on_drop(cx.listener(|wb, d: &DraggedTab, w, cx| wb.move_tab(d.index, 1, None, w, cx)))
            .into_any_element()
    }

    /// What a tab shows: its page, or its editor.
    pub(super) fn render_page(&mut self, page: Option<Page>, editor: Entity<EditorView>, cx: &mut Context<Self>) -> gpui::AnyElement {
        match page {
            Some(Page::Module(id)) => self.render_module_page(&id, cx).into_any_element(),
            Some(Page::Markdown(path)) => self.render_markdown_page(&path, cx).into_any_element(),
            Some(Page::Diff(diff)) => diff.into_any_element(),
            Some(Page::Settings) => self.render_settings_page(cx).into_any_element(),
            Some(Page::Merge(merge)) => merge.into_any_element(),
            Some(Page::Secrets { project, file }) => self.render_secrets_page(&project, &file, cx).into_any_element(),
            Some(Page::Resx(view)) => view.into_any_element(),
            None => editor.into_any_element(),
        }
    }

    pub(super) fn render_tabs(&mut self, at_bottom: bool, group: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::global(cx);
        let indicator_bottom = t.layout.tab_indicator_bottom;
        let active = self.visible[group];
        let markdown_source = active.and_then(|i| self.tabs.get(i)).is_some_and(|t| t.is_document() && t.path.as_deref().is_some_and(preview::is_markdown));
        let drop_color = t.c("focusBorder");
        let group_ix = group;
        let tab_els: Vec<_> = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, tab)| tab.group == group)
            .map(|(i, tab)| {
                let is_active = active == Some(i);
                let modified = tab.modified(cx);
                let fg = if is_active { t.c("tab.activeForeground") } else { t.c("tab.inactiveForeground") };
                let bg = if is_active { t.bg("tab.activeBackground") } else { t.bg("tab.inactiveBackground") };
                let hover = t.c("tab.hoverBackground");
                let close_hover = t.c("toolbar.hoverBackground");
                let indicator = t.m("tab.indicatorSize", 1.0);
                let group = SharedString::from(format!("tab-{i}"));
                let close = div()
                    .id(SharedString::from(format!("tab-close-{i}")))
                    .ml(t.edges("tab.closeMargin", 6.0).left)
                    .p(t.edges("tab.closePadding", 3.0).top)
                    .rounded(t.radius("tab.closeRadius", 4.0))
                    .hover(move |s| s.bg(close_hover))
                    .when(!is_active && !modified, |d| d.invisible().group_hover(group.clone(), |s| s.visible()))
                    .map(|d| {
                        if modified {
                            d.child(
                                div()
                                    .size(t.m("tab.closeIconSize", 12.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(div().size(t.m("tab.modifiedDotSize", 8.0)).rounded_full().bg(t.c("tab.modifiedIndicator"))),
                            )
                        } else {
                            d.child(icon("close", t.m("tab.closeIconSize", 12.0), fg))
                        }
                    })
                    .on_click(cx.listener(move |wb, _, w, cx| {
                        cx.stop_propagation();
                        wb.close_tab(i, w, cx)
                    }));
                let margin = t.edges("tab.margin", 0.0);
                let padding = t.edges("tab.padding", 12.0);
                let border = t.edges("tab.borderThickness", 0.0);
                div()
                    .id(SharedString::from(format!("tab-{i}")))
                    .debug_selector(|| format!("tab-{group_ix}-{}", tab.title))
                    .group(SharedString::from(format!("tab-{i}")))
                    .relative()
                    .h_full()
                    .min_w(t.m("tab.minWidth", 90.0))
                    .mt(margin.top)
                    .mb(margin.bottom)
                    .ml(margin.left)
                    .mr(margin.right)
                    .pl(padding.left)
                    .pr(padding.right)
                    .flex()
                    .items_center()
                    .bg(bg)
                    .rounded(t.radius("tab.radius", 0.0))
                    .border_color(if is_active { t.c("tab.activeBorder") } else { t.c("tab.border") })
                    .border_l(border.left)
                    .border_r(border.right)
                    .border_t(border.top)
                    .border_b(border.bottom)
                    .text_color(fg)
                    .text_size(t.m("tab.fontSize", 13.0))
                    .when(is_active, |d| d.font_weight(t.weight("tab.activeWeight", gpui::FontWeight::NORMAL)))
                    .when(t.italic("tab.style"), |d| d.italic())
                    .cursor_pointer()
                    .when(!is_active, |d| d.hover(move |s| s.bg(hover)))
                    .tooltip(tooltip(tab.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| tab.title.clone())))
                    .when(is_active && indicator > px(0.), |d| {
                        d.child(div().absolute().when(indicator_bottom, |d| d.bottom_0()).when(!indicator_bottom, |d| d.top_0()).left_0().right_0().h(indicator).rounded(t.radius("tab.indicatorRadius", 0.0)).bg(t.bg("tab.activeBorderTop")))
                    })
                    .child(div().whitespace_nowrap().child(tab.title.clone()))
                    .child(close)
                    .on_mouse_down(MouseButton::Middle, cx.listener(move |wb, _, w, cx| wb.close_tab(i, w, cx)))
                    .on_click(cx.listener(move |wb, _, w, cx| wb.activate(i, w, cx)))
                    // Drag a tab onto another to move it there (also into the other group).
                    .on_drag(DraggedTab { index: i, title: tab.title.clone() }, |d, _, _, cx| cx.new(|_| d.clone()))
                    .drag_over::<DraggedTab>(move |s, d, _, _| if d.index == i { s } else { s.border_l_2().border_color(drop_color) })
                    .on_drop(cx.listener(move |wb, d: &DraggedTab, w, cx| {
                        cx.stop_propagation();
                        wb.move_tab(d.index, group_ix, Some(i), w, cx);
                    }))
            })
            .collect();
        div()
            .id(SharedString::from(format!("tab-strip-{group}")))
            .debug_selector(|| format!("tab-strip-{group}"))
            .flex_none()
            .h(t.m("tab.height", 35.0))
            .w_full()
            .flex()
            .gap(t.m("tab.spacing", 0.0))
            .px(t.edges("tabStrip.padding", 0.0).left)
            .overflow_x_scroll()
            .bg(t.bg("editorGroupHeader.background"))
            .map(|d| {
                let b = px(f32::from(t.edges("editorGroupHeader.borderThickness", 1.0).bottom));
                if at_bottom { d.border_t(b) } else { d.border_b(b) }
            })
            .border_color(t.c("editorGroupHeader.border"))
            .children(tab_els)
            // Double click on the empty space after the tabs: a new untitled editor in this group.
            .child(div().flex_1().h_full().on_mouse_down(
                MouseButton::Left,
                cx.listener(move |wb, ev: &MouseDownEvent, w, cx| {
                    if ev.click_count == 2 {
                        if wb.focus_group != group_ix {
                            wb.focus_group(group_ix, w, cx);
                        }
                        wb.new_file(w, cx);
                    }
                }),
            ))
            // Empty space of the strip: to the end of this group.
            .on_drop(cx.listener(move |wb, d: &DraggedTab, w, cx| wb.move_tab(d.index, group_ix, None, w, cx)))
            .when(markdown_source, |d| {
                d.child(div().flex_none().h_full().flex().items_center().child(
                    div()
                        .id(SharedString::from(format!("md-preview-button-{group}")))
                        .mx_2()
                        .p(px(4.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .hover(|s| s.bg(t.c("toolbar.hoverBackground")))
                        .tooltip(tooltip(format!("Open Preview to the Side ({} opens it here)", crate::actions::key_label("ctrl-shift-v"))))
                        .child(icon("openPreview", px(16.), t.c("icon.foreground")))
                        .on_click(cx.listener(|wb, _, w, cx| wb.open_markdown_preview(true, w, cx))),
                ))
            })
    }

    /// The breadcrumbs of tab `ix` (the one `group` shows). Each crumb opens a dropdown: a folder's
    /// entries, the file's folder, or the symbols next to a symbol.
    pub(super) fn render_breadcrumbs(&self, ix: Option<usize>, group: usize, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let t = Theme::global(cx);
        let tab = ix.and_then(|i| self.tabs.get(i)).filter(|tab| tab.is_document())?;
        let path = tab.path.clone()?;
        let editor = tab.editor.clone();
        let symbols = editor.read(cx).symbol_path();
        let base = self.workspace.root().filter(|root| path.starts_with(root)).unwrap_or_default();
        let parts: Vec<String> = path.strip_prefix(&base).unwrap_or(&path).components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        let count = parts.len() + symbols.len();
        let icon_size = t.m("breadcrumb.iconSize", 12.0);
        let fg = t.c("breadcrumb.foreground");
        let active = t.c("breadcrumb.activeForeground");
        let file_parts = parts.len();
        let mut crumbs: Vec<gpui::AnyElement> = Vec::new();
        let mut dir = base;
        for (i, part) in parts.into_iter().enumerate() {
            // A folder lists its own entries (the one on the way to the file marked); the file its folder's.
            let source = if i + 1 == file_parts {
                CrumbSource::Dir { dir: path.parent().map(Path::to_path_buf).unwrap_or_default(), mark: path.clone() }
            } else {
                dir.push(&part);
                let next = path.components().nth(dir.components().count()).map(|c| dir.join(c));
                CrumbSource::Dir { dir: dir.clone(), mark: next.unwrap_or_default() }
            };
            let color = if i + 1 == count { active } else { fg };
            crumbs.extend((i > 0).then(|| icon("chevronRight", icon_size, fg).into_any_element()));
            crumbs.push(self.render_crumb(group, i, part, color, source, &editor, cx));
        }
        for (i, (name, ..)) in symbols.into_iter().enumerate() {
            let color = if file_parts + i + 1 == count { active } else { fg };
            crumbs.push(icon("chevronRight", icon_size, fg).into_any_element());
            crumbs.push(self.render_crumb(group, file_parts + i, name, color, CrumbSource::Symbol(i), &editor, cx));
        }
        Some(
            div()
                .flex_none()
                .h(t.m("breadcrumb.height", 22.0))
                .px(t.edges("breadcrumb.padding", 14.0).left)
                .flex()
                .items_center()
                .gap_1()
                .bg(t.bg("breadcrumb.background"))
                .border_b(px(f32::from(t.edges("breadcrumb.borderThickness", 0.0).bottom)))
                .border_color(t.c("breadcrumb.border"))
                .text_size(t.m("breadcrumb.fontSize", 12.0))
                .children(crumbs),
        )
    }

    /// Crumb `crumb` of `group`'s breadcrumbs, with its dropdown when that is open.
    #[allow(clippy::too_many_arguments)]
    fn render_crumb(&self, group: usize, crumb: usize, label: String, color: gpui::Hsla, source: CrumbSource, editor: &Entity<EditorView>, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = Theme::global(cx);
        let hover = t.c("toolbar.hoverBackground");
        let menu = self.crumb_menu.as_ref().filter(|m| m.group == group && m.crumb == crumb && m.editor == *editor);
        let dropdown = menu.map(|m| self.render_crumb_menu(m, cx));
        let editor = editor.clone();
        let selector = format!("crumb-{group}-{label}");
        div()
            .id(SharedString::from(format!("crumb-{group}-{crumb}")))
            .debug_selector(|| selector)
            .relative()
            .px(px(2.))
            .rounded(px(3.))
            .cursor_pointer()
            .when(menu.is_some(), |d| d.bg(hover))
            .hover(move |s| s.bg(hover))
            .text_color(color)
            .child(label)
            .on_mouse_down(MouseButton::Left, cx.listener(move |wb, ev: &MouseDownEvent, window, cx| {
                // Not the group's own handler: that would focus the editor, away from the dropdown.
                cx.stop_propagation();
                if wb.crumb_closed_at.take() == Some((group, crumb, ev.position)) {
                    return;
                }
                wb.open_crumb_menu(group, crumb, &source, &editor, window, cx);
            }))
            .children(dropdown)
            .into_any_element()
    }

    fn open_crumb_menu(&mut self, group: usize, crumb: usize, source: &CrumbSource, editor: &Entity<EditorView>, window: &mut Window, cx: &mut Context<Self>) {
        if self.focus_group != group {
            self.focus_group(group, window, cx);
        }
        let (entries, current) = match source {
            CrumbSource::Dir { dir, mark } => {
                let entries = self.crumb_dir_entries(dir);
                let current = entries.iter().position(|e| matches!(&e.target, CrumbTarget::Dir(p) | CrumbTarget::File(p) if p == mark));
                (entries, current)
            }
            CrumbSource::Symbol(i) => {
                let e = editor.read(cx);
                let at = e.symbol_path().get(*i).map(|s| s.2);
                let entries: Vec<CrumbEntry> = e.sibling_symbols(*i).into_iter().map(|(label, kind, p)| CrumbEntry { label, target: CrumbTarget::Symbol(kind, p) }).collect();
                let current = entries.iter().position(|e| matches!(e.target, CrumbTarget::Symbol(_, p) if Some(p) == at));
                (entries, current)
            }
        };
        let focus = cx.focus_handle();
        window.focus(&focus);
        let scroll = gpui::ScrollHandle::new();
        scroll.scroll_to_item(current.unwrap_or(0));
        self.crumb_menu = Some(CrumbMenu { group, crumb, editor: editor.clone(), entries, selected: current.unwrap_or(0), current, focus, scroll });
        cx.notify();
    }

    /// A folder's entries as the Explorer shows them (folders first, excludes left out).
    fn crumb_dir_entries(&self, dir: &Path) -> Vec<CrumbEntry> {
        self.workspace
            .list_dir(dir)
            .into_iter()
            .map(|(p, is_dir)| CrumbEntry {
                label: p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                target: if is_dir { CrumbTarget::Dir(p) } else { CrumbTarget::File(p) },
            })
            .collect()
    }

    fn render_crumb_menu(&self, m: &CrumbMenu, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = Theme::global(cx);
        let selection = t.c("menu.selectionBackground");
        let items: Vec<gpui::AnyElement> = m
            .entries
            .iter()
            .enumerate()
            .map(|(ix, e)| {
                let item = match &e.target {
                    CrumbTarget::Symbol(kind, _) => crate::menu::item(&t, ix, e.label.clone(), Some((*kind).into())),
                    target => {
                        let icon = crate::explorer::file_icon_view(&e.label, matches!(target, CrumbTarget::Dir(_)), false, px(14.), &t);
                        crate::menu::icon_item(&t, ix, icon, e.label.clone())
                    }
                };
                let selector = format!("crumb-item-{}", e.label);
                item.debug_selector(|| selector)
                    .when(ix == m.selected, |d| d.bg(selection))
                    .when(m.current == Some(ix), |d| d.font_weight(gpui::FontWeight::SEMIBOLD))
                    .on_click(cx.listener(move |wb, _, window, cx| wb.pick_crumb_entry(ix, window, cx)))
                    .into_any_element()
            })
            .collect();
        let panel = crate::menu::panel(&t, 180.)
            .id("crumb-menu")
            .debug_selector(|| "crumb-menu".into())
            // The editor under it must not take the clicks.
            .occlude()
            .max_h(px(400.))
            .overflow_y_scroll()
            .track_scroll(&m.scroll)
            .track_focus(&m.focus)
            .key_context("Menu")
            .on_action(cx.listener(|wb, _: &InputUp, _, cx| wb.move_crumb_selection(-1, cx)))
            .on_action(cx.listener(|wb, _: &InputDown, _, cx| wb.move_crumb_selection(1, cx)))
            .on_action(cx.listener(|wb, _: &Confirm, window, cx| {
                if let Some(ix) = wb.crumb_menu.as_ref().map(|m| m.selected) {
                    wb.pick_crumb_entry(ix, window, cx);
                }
            }))
            .on_action(cx.listener(|wb, _: &InputCancel, window, cx| {
                if let Some(m) = wb.crumb_menu.take() {
                    m.editor.update(cx, |e, _| e.focus(window));
                }
                cx.notify();
            }))
            .on_mouse_down_out(cx.listener(|wb, ev: &MouseDownEvent, _, cx| {
                if let Some(m) = wb.crumb_menu.take() {
                    wb.crumb_closed_at = Some((m.group, m.crumb, ev.position));
                    cx.notify();
                }
            }))
            .children(items)
            .when(m.entries.is_empty(), |d| d.child(div().px_3().py_1().text_color(t.c("foreground.muted")).child("Empty")));
        div()
            .absolute()
            .left_0()
            .top(gpui::relative(1.))
            .child(gpui::deferred(gpui::anchored().anchor(gpui::Corner::TopLeft).snap_to_window().child(panel)).with_priority(1))
            .into_any_element()
    }

    #[cfg(test)]
    pub(super) fn crumb_menu_labels(&self) -> Option<Vec<String>> {
        let m = self.crumb_menu.as_ref()?;
        Some(m.entries.iter().enumerate().map(|(i, e)| if m.current == Some(i) { format!("*{}", e.label) } else { e.label.clone() }).collect())
    }

    fn move_crumb_selection(&mut self, by: isize, cx: &mut Context<Self>) {
        let Some(m) = self.crumb_menu.as_mut().filter(|m| !m.entries.is_empty()) else { return };
        m.selected = m.selected.saturating_add_signed(by).min(m.entries.len() - 1);
        m.scroll.scroll_to_item(m.selected);
        cx.notify();
    }

    /// A file opens in the crumb's group, a folder shows its entries, a symbol is gone to.
    fn pick_crumb_entry(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.crumb_menu.as_ref().and_then(|m| m.entries.get(ix)).map(|e| e.target.clone()) else { return };
        match target {
            CrumbTarget::Dir(dir) => {
                let entries = self.crumb_dir_entries(&dir);
                if let Some(m) = self.crumb_menu.as_mut() {
                    (m.entries, m.selected, m.current) = (entries, 0, None);
                    m.scroll.scroll_to_item(0);
                    window.focus(&m.focus);
                }
            }
            CrumbTarget::File(path) => {
                let Some(m) = self.crumb_menu.take() else { return };
                if self.focus_group != m.group {
                    self.focus_group(m.group, window, cx);
                }
                self.open_file(path, None, window, cx);
                // It was open only in the other group: a view of it here too.
                if self.is_split() && self.active.is_some_and(|a| self.tabs[a].group != m.group) {
                    self.open_in_other_group(window, cx);
                }
            }
            CrumbTarget::Symbol(_, at) => {
                if let Some(m) = self.crumb_menu.take() {
                    m.editor.update(cx, |e, cx| {
                        e.go_to(at, cx);
                        e.focus(window);
                    });
                }
            }
        }
        cx.notify();
    }

    /// The welcome page; `welcome.order` (logo, title, shortcuts) and `welcome.shortcuts` (command ids).
    pub(super) fn render_welcome(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::global(cx);
        let commands = cx.global::<Commands>();
        let mut page = div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(t.m("welcome.spacing", 8.0))
            .bg(t.bg("editor.background"))
            // Double click on the empty editor area: a new untitled editor.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|wb, ev: &MouseDownEvent, w, cx| {
                    if ev.click_count == 2 {
                        wb.new_file(w, cx);
                    }
                }),
            );
        for part in &t.layout.welcome {
            match part.as_str() {
                "logo" => page = page.child(icon("logo", t.m("welcome.logoSize", 96.0), t.c("welcome.logo"))),
                "title" => {
                    page = page.child(
                        div()
                            .text_size(t.m("welcome.titleSize", 44.0))
                            .font_weight(t.weight("welcome.titleWeight", gpui::FontWeight::LIGHT))
                            .text_color(t.c("welcome.titleForeground"))
                            .child(t.layout.welcome_title.clone()),
                    )
                }
                "shortcuts" => {
                    for id in &t.layout.welcome_commands {
                        let Some(cmd) = commands.get(id) else { continue };
                        let key = pretty_keys(&window.keystroke_text_for(cmd.action.as_ref()));
                        let key = if key.contains("::") { String::new() } else { key };
                        let action = cmd.action.boxed_clone();
                        page = page.child(
                            div()
                                .id(SharedString::from(format!("welcome-{id}")))
                                .flex()
                                .gap_3()
                                .cursor_pointer()
                                .text_color(t.c("welcome.foreground"))
                                .hover(|s| s.text_color(t.c("welcome.titleForeground")))
                                .child(div().w(t.m("welcome.columnWidth", 160.0)).flex().justify_end().child(cmd.title.clone()))
                                .child(div().w(t.m("welcome.columnWidth", 160.0)).child(key))
                                // A double click on a shortcut runs it; it does not also open an editor.
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(move |_, w, cx| w.dispatch_action(action.boxed_clone(), cx)),
                        );
                    }
                }
                _ => {}
            }
        }
        page
    }
}

/// A breadcrumb's open dropdown.
pub(super) struct CrumbMenu {
    group: usize,
    /// The crumb it hangs from (its place in the row), of `editor`'s breadcrumbs.
    crumb: usize,
    editor: Entity<EditorView>,
    entries: Vec<CrumbEntry>,
    /// The entry Up/Down/Enter act on, and the one the crumb stands for (the file itself, say).
    selected: usize,
    current: Option<usize>,
    focus: FocusHandle,
    scroll: gpui::ScrollHandle,
}

struct CrumbEntry {
    label: String,
    target: CrumbTarget,
}

#[derive(Clone)]
enum CrumbTarget {
    Dir(PathBuf),
    File(PathBuf),
    Symbol(&'static str, Point),
}

/// What a crumb's dropdown lists: `dir`'s entries with `mark` marked, or the siblings of the `i`th symbol.
enum CrumbSource {
    Dir { dir: PathBuf, mark: PathBuf },
    Symbol(usize),
}
