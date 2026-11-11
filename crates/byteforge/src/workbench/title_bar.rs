//! Title bar, command center and menu bar.

use super::*;

impl Workbench {
    pub(super) fn render_title_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::global(cx);
        let lay = &t.layout;
        let custom = self.custom_titlebar;
        let at_bottom = lay.title_bar_at_bottom();
        let left = self.title_items(&lay.title_left, "l", at_bottom, window, cx);
        let center = self.title_items(&lay.title_center, "c", at_bottom, window, cx);
        let mut right = self.title_items(&lay.title_right, "r", at_bottom, window, cx);
        // A custom title bar always keeps its window buttons, even if a theme forgets to place them.
        if custom && !lay.contains_title_item("windowControls") {
            right.extend(self.title_items(&["windowControls".to_owned()], "w", at_bottom, window, cx));
        }
        let border = px(f32::from(t.edges("titleBar.borderThickness", 1.0).bottom));
        div()
            .id("title-bar")
            .relative()
            .flex_none()
            .h(t.m("titleBar.height", 36.0))
            .w_full()
            .flex()
            .items_center()
            .bg(t.bg("titleBar.background"))
            .map(|d| if at_bottom { d.border_t(border) } else { d.border_b(border) })
            .border_color(t.c("titleBar.border"))
            .when(custom, |d| {
                d.window_control_area(WindowControlArea::Drag).on_mouse_down(MouseButton::Left, |e: &MouseDownEvent, window, _| {
                    // On Windows the title bar is a native caption area: the OS handles drag and double-click.
                    if cfg!(windows) {
                        return;
                    }
                    // macOS drags the window itself; double-click does what the user picked in System Settings.
                    if cfg!(target_os = "macos") {
                        if e.click_count == 2 {
                            window.titlebar_double_click();
                        }
                        return;
                    }
                    if e.click_count == 2 {
                        window.zoom_window();
                    } else {
                        window.start_window_move();
                    }
                })
            })
            // The center zone is an overlay so it stays centered in the window, whatever the sides contain.
            .when(!center.is_empty(), |d| d.child(div().absolute().inset_0().flex().items_center().justify_center().gap(t.m("titleBar.spacing", 2.0)).children(center)))
            // Room for the macOS traffic lights, which sit over the top-left corner of the window.
            .when(custom && !at_bottom && cfg!(target_os = "macos"), |d| d.child(div().flex_none().w(px(76.))))
            .child(div().h_full().flex_1().min_w_0().flex().items_center().gap(t.m("titleBar.spacing", 2.0)).children(left))
            .child(div().h_full().flex().items_center().gap(t.m("titleBar.spacing", 2.0)).children(right))
    }

    /// Title bar parts: logo, menu, title, commandCenter, layoutControls (= toggleSideBar, togglePanel,
    /// themePicker), branch, windowControls, spacer, or any command (`command:<id>[:<icon>]`).
    pub(super) fn title_items(&mut self, names: &[String], zone: &str, upward: bool, window: &mut Window, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let t = Theme::global(cx);
        let fg = if window.is_window_active() { t.c("titleBar.foreground") } else { t.c("titleBar.inactiveForeground") };
        let hover = t.c("toolbar.hoverBackground");
        let icon_size = t.m("titleBar.iconSize", 16.0);
        let title_icon = |id: SharedString, name: &str, tip: String, active: bool| {
            let color = if active { t.c("titleIcon.activeForeground") } else { fg };
            div()
                .id(id)
                // Title bar controls occlude the drag area beneath them; otherwise Windows reports the point as
                // the caption (the first window-control area found) and swallows the click to move the window.
                .occlude()
                .flex_none()
                .p(t.edges("titleIcon.padding", 5.0).top)
                .rounded(t.radius("titleIcon.radius", 4.0))
                .opacity(if active { 1.0 } else { t.n("titleIcon.inactiveOpacity", 0.75) })
                .cursor_pointer()
                .hover(move |s| s.bg(hover).opacity(1.0))
                .tooltip(tooltip(tip))
                .child(icon(name, icon_size, color))
        };
        let expanded: Vec<&str> = names
            .iter()
            .flat_map(|n| if n == "layoutControls" { vec!["toggleSideBar", "togglePanel", "themePicker"] } else { vec![n.as_str()] })
            .collect();
        let mut out = Vec::new();
        for (n, name) in expanded.into_iter().enumerate() {
            let id = SharedString::from(format!("tb-{zone}-{n}"));
            match name {
                "logo" => out.push(div().flex_none().pl_3().pr_2().child(icon("logo", t.m("titleBar.logoSize", 16.0), t.c("titleBar.logo"))).into_any_element()),
                // macOS shows the menus in its own menu bar (see `crate::app_menu`).
                "menu" => {
                    if !cfg!(target_os = "macos") {
                        out.push(self.render_menu_bar(upward, fg, cx).into_any_element());
                    }
                }
                "title" => {
                    let text = match self.active.and_then(|i| self.tabs.get(i)) {
                        Some(tab) => format!("{} — {}", tab.title, self.workspace.root().and_then(|r| r.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| "ByteForge".into())),
                        None => "ByteForge".into(),
                    };
                    out.push(div().px_2().min_w_0().overflow_hidden().whitespace_nowrap().text_size(t.m("titleBar.titleFontSize", 12.0)).text_color(fg).child(text).into_any_element());
                }
                "commandCenter" => {
                    if f32::from(window.viewport_size().width) >= t.n("commandCenter.minWindowWidth", 820.0) {
                        out.push(self.render_command_center(cx));
                    }
                }
                "branch" => {
                    if self.scm.read(cx).git().is_some() {
                        let branch = self.scm.read(cx).status().branch.clone().unwrap_or_else(|| "(detached)".into());
                        out.push(
                            div()
                                .id(id)
                                .occlude()
                                .flex_none()
                                .px_2()
                                .py(px(3.))
                                .flex()
                                .items_center()
                                .gap_1()
                                .rounded(t.radius("titleIcon.radius", 4.0))
                                .text_size(t.m("titleBar.titleFontSize", 12.0))
                                .text_color(fg)
                                .cursor_pointer()
                                .hover(move |s| s.bg(hover))
                                .tooltip(tooltip("Checkout branch…"))
                                .child(icon("sourceControl", icon_size, fg))
                                .child(branch)
                                .on_click(cx.listener(|wb, _, w, cx| wb.open_branch_picker(w, cx)))
                                .into_any_element(),
                        );
                    }
                }
                "toggleSideBar" => {
                    let icon_name = if t.layout.right_of_editor("sideBar") { "layoutSideBarRight" } else { "layoutSideBar" };
                    out.push(title_icon(id, icon_name, format!("Toggle Side Bar ({})", crate::actions::key_label("ctrl-b")), self.side_visible).on_click(cx.listener(|wb, _, _, cx| {
                        wb.side_visible = !wb.side_visible;
                        cx.notify();
                    })).into_any_element());
                }
                "togglePanel" => out.push(title_icon(id, "layoutPanel", format!("Toggle Panel ({})", crate::actions::key_label("ctrl-j")), self.panel_visible).on_click(cx.listener(|wb, _, _, cx| {
                    wb.panel_visible = !wb.panel_visible;
                    cx.notify();
                })).into_any_element()),
                "themePicker" => out.push(title_icon(id, "palette", format!("Color Theme ({})", crate::actions::key_label("ctrl-k")), false).on_click(cx.listener(|wb, _, w, cx| wb.open_palette(Mode::Themes, w, cx))).into_any_element()),
                "windowControls" => {
                    if self.custom_titlebar && !cfg!(target_os = "macos") {
                        out.push(render_window_controls(window, cx).into_any_element());
                    }
                }
                "spacer" => out.push(div().flex_1().into_any_element()),
                other => {
                    if let Some((action, title, icon_name)) = command_button(other, cx) {
                        out.push(title_icon(id, &icon_name, title, false).on_click(move |_, w, cx| w.dispatch_action(action.boxed_clone(), cx)).into_any_element());
                    }
                }
            }
        }
        out
    }

    pub(super) fn render_command_center(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = Theme::global(cx);
        let root_name = self.workspace.root().and_then(|r| r.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| "ByteForge".into());
        let cc_hover_bg = t.c("commandCenter.hoverBackground");
        let cc_hover_border = t.c("commandCenter.hoverBorder");
        div()
            .id("command-center")
            .occlude()
            .flex_shrink()
            .min_w(px(120.))
            .w(t.m("commandCenter.width", 380.0))
            .h(t.m("commandCenter.height", 24.0))
            .flex()
            .items_center()
            .justify_center()
            .gap_2()
            .bg(t.bg("commandCenter.background"))
            .border(px(f32::from(t.edges("commandCenter.borderThickness", 1.0).top)))
            .border_color(t.c("commandCenter.border"))
            .rounded(t.radius("commandCenter.radius", 6.0))
            .text_color(t.c("commandCenter.foreground"))
            .text_size(t.m("commandCenter.fontSize", 12.0))
            .cursor_pointer()
            .hover(move |s| s.bg(cc_hover_bg).border_color(cc_hover_border))
            .tooltip(tooltip(format!("Search files by name ({})", crate::actions::key_label("ctrl-p"))))
            .child(icon("search", t.m("commandCenter.iconSize", 13.0), t.c("commandCenter.foreground")))
            .child(root_name)
            .on_click(cx.listener(|wb, _, w, cx| {
                let mode = if wb.workspace.root().is_some() { Mode::Files } else { Mode::Commands };
                wb.open_palette(mode, w, cx)
            }))
            .into_any_element()
    }

    /// The menu bar; `layout."menuBar.menus"` picks and orders the menus.
    pub(super) fn render_menu_bar(&mut self, upward: bool, fg: gpui::Hsla, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::global(cx);
        let hover = t.c("toolbar.hoverBackground");
        let mut row = div().flex_none().flex().items_center();
        for name in &t.layout.menus {
            let Some(i) = MENUS.iter().position(|(n, _)| n.eq_ignore_ascii_case(name)) else { continue };
            let open = self.menu_open == Some(i);
            let dropdown = open.then(|| self.render_menu_dropdown(i, upward, cx));
            row = row.child(
                div()
                    .id(SharedString::from(format!("menu-{name}")))
                    .occlude()
                    .relative()
                    .px(t.edges("menuBarItem.padding", 8.0).left)
                    .py(t.edges("menuBarItem.padding", 4.0).top)
                    .rounded(t.radius("menuBarItem.radius", 4.0))
                    .text_size(t.m("menuBar.fontSize", 13.0))
                    .text_color(fg)
                    .cursor_pointer()
                    .when(open, |d| d.bg(hover))
                    .hover(move |s| s.bg(hover))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |wb, _, _, cx| {
                        wb.menu_open = if wb.menu_open == Some(i) { None } else { Some(i) };
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .on_hover(cx.listener(move |wb, hovered: &bool, _, cx| {
                        if *hovered && wb.menu_open.is_some() && wb.menu_open != Some(i) {
                            wb.menu_open = Some(i);
                            cx.notify();
                        }
                    }))
                    .child(MENUS[i].0)
                    .children(dropdown),
            );
        }
        row
    }

    /// Dropdown anchored to its menu button (opens upwards when the title bar is at the bottom).
    pub(super) fn render_menu_dropdown(&self, i: usize, upward: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = Theme::global(cx);
        let (_, entries) = MENUS[i];
        let commands = cx.global::<Commands>();
        let items: Vec<gpui::AnyElement> = entries
            .iter()
            .enumerate()
            .filter_map(|(ix, id)| {
                if *id == "-" {
                    return Some(crate::menu::separator(&t).into_any_element());
                }
                let cmd = commands.get(id)?;
                let action = cmd.action.boxed_clone();
                Some(
                    crate::menu::item(&t, ix, cmd.title.clone(), Some(default_key(id).into()))
                        .on_click(cx.listener(move |wb, _, w, cx| {
                            wb.menu_open = None;
                            w.dispatch_action(action.boxed_clone(), cx);
                            cx.notify();
                        }))
                        .into_any_element(),
                )
            })
            .collect();
        let menu = crate::menu::panel(&t, 240.)
            .on_mouse_down_out(cx.listener(|wb, _, _, cx| {
                wb.menu_open = None;
                cx.notify();
            }))
            .children(items);
        let corner = if upward { gpui::Corner::BottomLeft } else { gpui::Corner::TopLeft };
        div()
            .absolute()
            .left_0()
            .map(|d| if upward { d.top_0() } else { d.top(gpui::relative(1.)) })
            .child(gpui::deferred(gpui::anchored().anchor(corner).snap_to_window().child(menu)).with_priority(2))
            .into_any_element()
    }
}

pub(super) fn render_window_controls(window: &Window, cx: &App) -> impl IntoElement + use<> {
    let t = Theme::global(cx);
    let maximized = window.is_maximized();
    let win_w = t.m("windowControl.width", 46.0);
    let wc_icon = t.m("windowControl.iconSize", 14.0);
    let wc_fg = t.c("windowControl.foreground");
    let wc_hover = t.c("windowControl.hoverBackground");
    let close_hover = t.c("windowControl.closeHoverBackground");
    let close_fg = t.c("windowControl.closeHoverForeground");
    div()
        .flex_none()
        .flex()
        .h_full()
        .child(
            div().id("wc-min").occlude().w(win_w).h_full().flex().items_center().justify_center().hover(move |s| s.bg(wc_hover))
                .window_control_area(WindowControlArea::Min)
                .child(icon("windowMinimize", wc_icon, wc_fg))
                .on_click(|_, window, _| window.minimize_window()),
        )
        .child(
            div().id("wc-max").occlude().w(win_w).h_full().flex().items_center().justify_center().hover(move |s| s.bg(wc_hover))
                .window_control_area(WindowControlArea::Max)
                .child(icon(if maximized { "windowRestore" } else { "windowMaximize" }, wc_icon, wc_fg))
                .on_click(|_, window, _| window.zoom_window()),
        )
        .child(
            div().id("wc-close").occlude().group("wc-close").w(win_w).h_full().flex().items_center().justify_center().hover(move |s| s.bg(close_hover))
                .window_control_area(WindowControlArea::Close)
                .child(icon("windowClose", wc_icon, wc_fg).group_hover("wc-close", move |s| s.text_color(close_fg)))
                .on_click(|_, window, cx| window.dispatch_action(Box::new(Quit), cx)),
        )
}
