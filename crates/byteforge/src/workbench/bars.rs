//! Activity bar and side bar.

use super::*;

impl Workbench {
    /// The activity bar: vertical in the main row, or horizontal when placed in `sideBar.order`.
    /// Items: explorer, search, scm, modules, settings, spacer, or any command (`command:<id>[:<icon>]`).
    pub(super) fn render_activity_bar(&mut self, right: bool, horizontal: bool, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::global(cx);
        let (item_w, item_h, icon_size) = if horizontal {
            (t.m("activityBar.horizontalItemWidth", 40.0), t.m("activityBar.horizontalHeight", 36.0), t.m("activityBar.horizontalIconSize", 18.0))
        } else {
            (t.m("activityBar.width", 48.0), t.m("activityBar.itemHeight", 48.0), t.m("activityBar.iconSize", 24.0))
        };
        let indicator = if horizontal {
            if t.data.metrics.contains_key("activityBar.indicatorBottom.border") {
                t.edges("activityBar.indicatorBottom.border", 0.0)
            } else {
                gpui::Edges { bottom: px(2.), ..Default::default() }
            }
        } else if right {
            t.edges("activityBar.indicatorRight.border", 0.0)
        } else {
            t.edges("activityBar.indicatorLeft.border", 2.0)
        };
        let active_bg = t.bg("activityBar.activeBackground");
        let hover_bg = t.c("activityBar.hoverBackground");
        let badge_size = t.n("activityBarBadge.size", 16.0);
        let badge_bg = t.bg("activityBarBadge.background");
        let badge_fg = t.c("activityBarBadge.foreground");
        let badge = move |count: usize| {
            (count > 0).then(|| {
                div()
                    .absolute()
                    .bottom(px(f32::from(item_h) / 2.0 - f32::from(icon_size) / 2.0 - badge_size / 3.0))
                    .right(px(f32::from(item_w) / 2.0 - f32::from(icon_size) / 2.0 - badge_size / 2.0))
                    .min_w(px(badge_size))
                    .h(px(badge_size))
                    .px(px(3.))
                    .rounded(px(badge_size / 2.0))
                    .bg(badge_bg)
                    .text_color(badge_fg)
                    .text_size(px(badge_size * 0.62))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(if count > 999 { "999+".to_owned() } else { count.to_string() })
            })
        };
        // Optional text under each icon (vertical bar only) and a horizontal inset that turns full-width items
        // into rounded tiles.
        let label_size = if horizontal { 0.0 } else { t.n("activityBar.labelSize", 0.0) };
        let inset = if horizontal { px(0.) } else { t.m("activityBar.itemInset", 0.0) };
        let active_icon = if t.has("activityBar.activeIconForeground") { t.c("activityBar.activeIconForeground") } else { t.c("activityBar.foreground") };
        let item = |id: SharedString, name: &str, tip: String, label: &str, active: bool| {
            let color = if active { t.c("activityBar.foreground") } else { t.c("activityBar.inactiveForeground") };
            let icon_color = if active { active_icon } else { color };
            let hover_fg = t.c("activityBar.foreground");
            div()
                .id(id)
                .flex_none()
                .w(item_w - inset * 2.0)
                .h(item_h)
                .mx(inset)
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(3.))
                .relative()
                .text_color(color)
                .rounded(t.radius("activityBar.itemRadius", 0.0))
                .cursor_pointer()
                .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
                .when(active, |d| {
                    d.bg(active_bg)
                        .border_color(t.c("activityBar.activeBorder"))
                        .border_l(indicator.left)
                        .border_r(indicator.right)
                        .border_t(indicator.top)
                        .border_b(indicator.bottom)
                })
                .tooltip(tooltip(tip))
                .child(icon(name, icon_size, icon_color))
                .when(label_size > 0.0 && !label.is_empty(), |d| {
                    d.child(div().text_size(px(label_size)).line_height(px(label_size * 1.2)).font_weight(gpui::FontWeight::MEDIUM).child(label.to_owned()))
                })
        };
        let side_visible = self.side_visible;
        let side = self.side;
        let scm_count = self.scm.read(cx).status().count();
        let mut bar = div()
            .flex_none()
            .flex()
            .gap(t.m("activityBar.spacing", 0.0))
            .bg(t.bg("activityBar.background"))
            .border_color(t.c("activityBar.border"))
            .map(|d| {
                if horizontal {
                    d.w_full().h(item_h).flex_row().px(t.edges("activityBar.horizontalPadding", 4.0).left).border_b_1()
                } else {
                    d.w(item_w).h_full().flex_col().when(right, |d| d.border_l_1()).when(!right, |d| d.border_r_1())
                }
            });
        // Module views: where the layout names them, else before the first spacer (or at the end).
        let module_views: Vec<(String, String, String, u64)> = self.module_views.iter().map(|v| (v.id.clone(), v.title.clone(), v.icon.clone(), v.badge)).collect();
        let module_item = |n: String, (view_id, title, icon_name, count): &(String, String, String, u64), cx: &mut Context<Self>| {
            let active = self.module_view_active(view_id);
            let view_id = view_id.clone();
            item(SharedString::from(format!("act-m-{n}-{}", if horizontal { "h" } else { "v" })), icon_name, title.clone(), title, active)
                .children(badge(*count as usize))
                .on_click(cx.listener(move |wb, _, _, cx| wb.toggle_module_view(&view_id, cx)))
        };
        let mut unplaced: Vec<&(String, String, String, u64)> = module_views.iter().filter(|v| !t.layout.activity_items.iter().any(|i| i == &v.0)).collect();
        for (n, name) in t.layout.activity_items.iter().enumerate() {
            let id = SharedString::from(format!("act-{n}-{}", if horizontal { "h" } else { "v" }));
            if let Some(v) = module_views.iter().find(|v| &v.0 == name) {
                bar = bar.child(module_item(n.to_string(), v, cx));
                continue;
            }
            if name == "spacer" {
                for (k, v) in std::mem::take(&mut unplaced).into_iter().enumerate() {
                    bar = bar.child(module_item(format!("{n}-{k}"), v, cx));
                }
            }
            let view = match name.as_str() {
                "explorer" => Some((SideView::Explorer, "files", "Explorer (Ctrl+Shift+E)", "Files")),
                "search" => Some((SideView::Search, "search", "Search (Ctrl+Shift+F)", "Search")),
                "scm" => Some((SideView::SourceControl, "sourceControl", "Source Control (Ctrl+Shift+G)", "Git")),
                "debug" => Some((SideView::Debug, "debug", "Run and Debug (Ctrl+Shift+D)", "Debug")),
                "testing" | "tests" => Some((SideView::Testing, "beaker", "Testing", "Tests")),
                "solution" => Some((SideView::Solution, "fileSolution", "Solution", "Solution")),
                "modules" => Some((SideView::Modules, "puzzle", "Modules", "Modules")),
                _ => None,
            };
            if let Some((view, icon_name, tip, label)) = view {
                let mut el = item(id, icon_name, tip.into(), label, side_visible && side == view);
                if view == SideView::SourceControl {
                    el = el.children(badge(scm_count));
                }
                bar = bar.child(el.on_click(cx.listener(move |wb, _, w, cx| wb.toggle_side(view, w, cx))));
                continue;
            }
            match name.as_str() {
                "spacer" => bar = bar.child(div().flex_1()),
                "settings" => {
                    bar = bar.child(item(id, "settings", "Settings (Ctrl+,)".into(), "", false).on_click(cx.listener(|_, _, w, cx| w.dispatch_action(Box::new(OpenSettings), cx))));
                }
                other => {
                    if let Some((action, title, icon_name)) = command_button(other, cx) {
                        bar = bar.child(item(id, &icon_name, title, "", false).on_click(move |_, w, cx| w.dispatch_action(action.boxed_clone(), cx)));
                    }
                }
            }
        }
        for (k, v) in unplaced.into_iter().enumerate() {
            bar = bar.child(module_item(format!("end-{k}"), v, cx));
        }
        bar
    }

    /// The side bar; `sideBar.order` arranges header, content and (optionally) a horizontal activity bar.
    pub(super) fn render_side_bar(&mut self, right: bool, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::global(cx);
        let title: SharedString = match self.side {
            SideView::Explorer => "EXPLORER".into(),
            SideView::Search => "SEARCH".into(),
            SideView::SourceControl => "SOURCE CONTROL".into(),
            SideView::Debug => "RUN AND DEBUG".into(),
            SideView::Testing => "TESTING".into(),
            SideView::Solution => "SOLUTION".into(),
            SideView::Module => self.module_views.iter().find(|v| Some(&v.id) == self.module_view.as_ref()).map_or("MODULE".into(), |v| v.title.to_uppercase().into()),
            SideView::Modules => "MODULES".into(),
        };
        let mut content: Option<gpui::AnyElement> = Some(match self.side {
            SideView::Explorer => self.explorer.clone().into_any_element(),
            SideView::Search => self.search.clone().into_any_element(),
            SideView::SourceControl => self.scm.clone().into_any_element(),
            SideView::Debug => self.render_debug_view(cx),
            SideView::Testing => self.tests.clone().into_any_element(),
            SideView::Solution => self.solution.clone().into_any_element(),
            SideView::Module => self.render_module_view(cx),
            SideView::Modules => self.render_modules(cx).into_any_element(),
        });
        let sash_hover = t.c("sash.hoverBorder");
        let width = self.side_width;
        let sash = div()
            .id("side-sash")
            .absolute()
            .top_0()
            .bottom_0()
            .w(px(5.))
            .when(right, |d| d.left(px(-2.)))
            .when(!right, |d| d.right(px(-2.)))
            .cursor(gpui::CursorStyle::ResizeLeftRight)
            .hover(move |s| s.bg(sash_hover))
            .on_mouse_down(MouseButton::Left, cx.listener(move |wb, e: &MouseDownEvent, _, cx| {
                wb.sash = Some(Sash::Side { start_x: f32::from(e.position.x), start_w: width, right });
                cx.stop_propagation();
            }));
        let mut col = div()
            .relative()
            .flex_none()
            .w(px(width))
            .h_full()
            .flex()
            .flex_col()
            .bg(t.bg("sideBar.background"))
            .text_color(t.c("sideBar.foreground"))
            .border_color(t.c("sideBar.border"))
            .when(right, |d| d.border_l_1())
            .when(!right, |d| d.border_r_1());
        for part in &t.layout.side_bar {
            match part.as_str() {
                "header" => {
                    col = col.child(
                        div()
                            .flex_none()
                            .h(t.m("sideBarHeader.height", 35.0))
                            .px(t.edges("sideBarHeader.padding", 20.0).left)
                            .flex()
                            .items_center()
                            .text_size(t.m("sideBarTitle.fontSize", 11.0))
                            .font_weight(t.weight("sideBarTitle.weight", gpui::FontWeight::NORMAL))
                            .text_color(t.c("sideBarTitle.foreground"))
                            .child(title.clone()),
                    )
                }
                "content" => {
                    if let Some(c) = content.take() {
                        col = col.child(div().flex_1().min_h_0().child(c));
                    }
                }
                "activityBar" => col = col.child(self.render_activity_bar(false, true, cx)),
                _ => {}
            }
        }
        // The view itself can't be dropped, only moved.
        if let Some(c) = content {
            col = col.child(div().flex_1().min_h_0().child(c));
        }
        col.child(sash)
    }
}
