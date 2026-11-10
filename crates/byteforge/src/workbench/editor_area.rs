//! The editor area: tab strips, breadcrumbs, pages and the welcome screen.

use super::*;

impl Workbench {
    /// The editor column; `editor.order` arranges tabs, breadcrumbs, content and the panel.
    pub(super) fn render_main(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::global(cx);
        let lay = &t.layout;
        let split = self.is_split();
        let tabs_at_bottom = lay.tabs_below_content();
        let show_tabs = lay.editor.iter().any(|p| p == "tabs");
        let mut content: Option<gpui::AnyElement> = Some(if split {
            // Two groups side by side, each with its own tab strip.
            let border = t.c_or("editorGroup.border", 0x80808040);
            let groups: Vec<_> = (0..2)
                .map(|g| {
                    let shown = self.visible[g].and_then(|i| self.tabs.get(i)).map(|t| (t.editor.clone(), t.page.clone()));
                    let editor: Option<gpui::AnyElement> = shown.map(|(editor, page)| self.render_page(page, editor, cx));
                    let tabs = show_tabs.then(|| self.render_tabs(tabs_at_bottom, g, cx));
                    let body = div().flex_1().min_h_0().children(editor);
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
                            if tabs_at_bottom { d.child(body).children(tabs) } else { d.children(tabs).child(body) }
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
                "breadcrumbs" => col = col.children(self.render_breadcrumbs(cx)),
                "content" => {
                    if let Some(c) = content.take() {
                        col = col.child(div().flex_1().min_h_0().child(c));
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
            col = col.child(div().flex_1().min_h_0().child(c));
        }
        col
    }

    /// What a tab shows: its page, or its editor.
    pub(super) fn render_page(&mut self, page: Option<Page>, editor: Entity<EditorView>, cx: &mut Context<Self>) -> gpui::AnyElement {
        match page {
            Some(Page::Module(id)) => self.render_module_page(&id, cx).into_any_element(),
            Some(Page::Markdown(path)) => self.render_markdown_page(&path, cx).into_any_element(),
            None => editor.into_any_element(),
        }
    }

    pub(super) fn render_tabs(&mut self, at_bottom: bool, group: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::global(cx);
        let indicator_bottom = t.layout.tab_indicator_bottom;
        let active = self.visible[group];
        let markdown_source = active.and_then(|i| self.tabs.get(i)).is_some_and(|t| t.page.is_none() && t.path.as_deref().is_some_and(preview::is_markdown));
        let tab_els: Vec<_> = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, tab)| tab.group == group)
            .map(|(i, tab)| {
                let is_active = active == Some(i);
                let modified = tab.editor.read(cx).is_modified();
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
            })
            .collect();
        div()
            .id(SharedString::from(format!("tab-strip-{group}")))
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
            .when(markdown_source, |d| {
                d.child(div().flex_1()).child(div().flex_none().h_full().flex().items_center().child(
                    div()
                        .id(SharedString::from(format!("md-preview-button-{group}")))
                        .mx_2()
                        .p(px(4.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .hover(|s| s.bg(t.c("toolbar.hoverBackground")))
                        .tooltip(tooltip("Open Preview to the Side (Ctrl+Shift+V opens it here)"))
                        .child(icon("openPreview", px(16.), t.c("icon.foreground")))
                        .on_click(cx.listener(|wb, _, w, cx| wb.open_markdown_preview(true, w, cx))),
                ))
            })
    }

    pub(super) fn render_breadcrumbs(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let t = Theme::global(cx);
        let path = self.active.and_then(|i| self.tabs.get(i)).and_then(|tab| tab.path.clone())?;
        let display = match self.workspace.root() {
            Some(root) => path.strip_prefix(&root).map(Path::to_path_buf).unwrap_or(path.clone()),
            None => path.clone(),
        };
        let parts: Vec<String> = display.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        let count = parts.len();
        let icon_size = t.m("breadcrumb.iconSize", 12.0);
        let fg = t.c("breadcrumb.foreground");
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
                .children(parts.into_iter().enumerate().flat_map(move |(i, part)| {
                    let color = if i + 1 == count { t.c("breadcrumb.activeForeground") } else { fg };
                    let sep = (i > 0).then(|| icon("chevronRight", icon_size, fg).into_any_element());
                    sep.into_iter().chain(std::iter::once(div().text_color(color).child(part).into_any_element()))
                })),
        )
    }

    /// The welcome page; `welcome.order` (logo, title, shortcuts) and `welcome.shortcuts` (command ids).
    pub(super) fn render_welcome(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::global(cx);
        let commands = cx.global::<Commands>();
        let mut page = div().size_full().flex().flex_col().items_center().justify_center().gap(t.m("welcome.spacing", 8.0)).bg(t.bg("editor.background"));
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
