//! The bottom panel and the status bar.

use super::*;

impl Workbench {
    pub(super) fn render_panel(&mut self, place: PanelPlace, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::global(cx);
        let problems = self.problems(cx);
        let tab = |id: SharedString, label: String, active: bool| {
            let border = t.edges("panelTab.indicator.border", 1.0);
            div()
                .id(id)
                .px(t.edges("panelTab.padding", 4.0).left)
                .py(t.edges("panelTab.padding", 6.0).top)
                .mx(t.edges("panelTab.margin", 6.0).left)
                .rounded(t.radius("panelTab.radius", 0.0))
                .text_size(t.m("panelTab.fontSize", 11.0))
                .cursor_pointer()
                .text_color(if active { t.c("panelTitle.activeForeground") } else { t.c("panelTitle.inactiveForeground") })
                .when(active, |d| d.bg(t.bg("panelTitle.activeBackground")).border_b(border.bottom).border_color(t.c("panelTitle.activeBorder")))
                .child(label)
        };
        let (height, width) = (self.panel_height, self.panel_width);
        let sash_hover = t.c("sash.hoverBorder");
        let content: gpui::AnyElement = match self.panel {
            PanelView::Output => self.output.clone().into_any_element(),
            PanelView::DebugConsole => self.render_debug_console(cx),
            PanelView::References => self.render_references(cx),
            PanelView::Terminal => match self.terminals.get(self.active_terminal) {
                Some((terminal, _)) => terminal.clone().into_any_element(),
                None => div().p_3().text_color(t.c("foreground.muted")).child(format!("No terminal is open. Press {} to start one.", crate::actions::key_label("ctrl-shift-`"))).into_any_element(),
            },
            PanelView::Problems => {
                let root = self.workspace.root();
                let icon_size = t.m("panel.iconSize", 14.0);
                let rows: Vec<_> = problems
                    .iter()
                    .take(2000)
                    .enumerate()
                    .map(|(i, (path, d))| {
                        let (name, color_key) = match d.severity {
                            bf_lsp::Severity::Error => ("error", "problemsErrorIcon.foreground"),
                            bf_lsp::Severity::Warning => ("warning", "problemsWarningIcon.foreground"),
                            _ => ("info", "problemsInfoIcon.foreground"),
                        };
                        let rel = root.as_ref().and_then(|r| path.strip_prefix(r).ok()).unwrap_or(path).display().to_string();
                        let (p, at) = (path.clone(), d.start);
                        div()
                            .id(i)
                            .px_3()
                            .py(px(2.))
                            .flex()
                            .items_center()
                            .gap_2()
                            .cursor_pointer()
                            .hover(|s| s.bg(t.c("list.hoverBackground")))
                            .child(icon(name, icon_size, t.c(color_key)))
                            .child(div().whitespace_nowrap().child(d.message.clone()))
                            .child(div().text_color(t.c("foreground.muted")).whitespace_nowrap().child(format!(
                                "{}{rel} [Ln {}, Col {}]",
                                d.code.as_ref().map(|c| format!("{c}  ")).unwrap_or_default(),
                                d.start.line + 1,
                                d.start.col + 1
                            )))
                            .on_click(cx.listener(move |wb, _, w, cx| wb.open_file(p.clone(), Some(at), w, cx)))
                    })
                    .collect();
                div()
                    .id("problems")
                    .size_full()
                    .overflow_y_scroll()
                    .when(rows.is_empty(), |d| d.child(div().p_3().text_color(t.c("foreground.muted")).child("No problems have been detected in the open files.")))
                    .children(rows)
                    .into_any_element()
            }
        };
        let errors = problems.iter().filter(|(_, d)| d.severity == bf_lsp::Severity::Error).count();
        let thickness = px(f32::from(t.edges("panel.borderThickness", 1.0).top));
        let horizontal = matches!(place, PanelPlace::Top | PanelPlace::Bottom);
        let sash = div()
            .id("panel-sash")
            .absolute()
            .hover(move |s| s.bg(sash_hover))
            .map(|d| match place {
                PanelPlace::Bottom => d.top(px(-2.)).left_0().right_0().h(px(5.)),
                PanelPlace::Top => d.bottom(px(-2.)).left_0().right_0().h(px(5.)),
                PanelPlace::Left => d.right(px(-2.)).top_0().bottom_0().w(px(5.)),
                PanelPlace::Right => d.left(px(-2.)).top_0().bottom_0().w(px(5.)),
            })
            .cursor(if horizontal { gpui::CursorStyle::ResizeUpDown } else { gpui::CursorStyle::ResizeLeftRight })
            .on_mouse_down(MouseButton::Left, cx.listener(move |wb, e: &MouseDownEvent, _, cx| {
                wb.sash = Some(match place {
                    PanelPlace::Bottom | PanelPlace::Top => Sash::Panel { start_y: f32::from(e.position.y), start_h: height, top: place == PanelPlace::Top },
                    PanelPlace::Left | PanelPlace::Right => Sash::PanelSide { start_x: f32::from(e.position.x), start_w: width, right: place == PanelPlace::Right },
                });
                cx.stop_propagation();
            }));
        let mut header = div().flex_none().h(t.m("panelHeader.height", 35.0)).px(t.edges("panelHeader.padding", 8.0).left).flex().items_center();
        // The Debug Console is always available, even when a theme lists the tabs without it.
        // The Debug Console and the terminal are always available, even when a theme lists the tabs without them.
        let mut panel_tabs = t.layout.panel_tabs.clone();
        for required in ["debugConsole", "terminal"] {
            if !panel_tabs.iter().any(|n| n == required) {
                panel_tabs.push(required.into());
            }
        }
        // References shows up once there's a result.
        if self.references.is_some() && !panel_tabs.iter().any(|n| n == "references") {
            panel_tabs.push("references".into());
        }
        for name in &panel_tabs {
            match name.as_str() {
                "references" => {
                    let count = self.references.as_ref().map_or(0, |r| r.items.len());
                    header = header.child(
                        tab("ptab-references".into(), format!("REFERENCES ({count})"), self.panel == PanelView::References)
                            .on_click(cx.listener(|wb, _, _, cx| wb.show_panel(PanelView::References, cx))),
                    );
                }
                "problems" => {
                    let label = format!("PROBLEMS{}", if errors > 0 { format!(" ({errors})") } else { String::new() });
                    header = header.child(tab("ptab-problems".into(), label, self.panel == PanelView::Problems).on_click(cx.listener(|wb, _, _, cx| wb.show_panel(PanelView::Problems, cx))));
                }
                "output" => {
                    header = header.child(tab("ptab-output".into(), "OUTPUT".into(), self.panel == PanelView::Output).on_click(cx.listener(|wb, _, _, cx| wb.show_panel(PanelView::Output, cx))));
                }
                "debugConsole" => {
                    header = header.child(
                        tab("ptab-debug".into(), "DEBUG CONSOLE".into(), self.panel == PanelView::DebugConsole)
                            .on_click(cx.listener(|wb, _, _, cx| wb.show_panel(PanelView::DebugConsole, cx))),
                    );
                }
                "terminal" => {
                    header = header.child(
                        tab("ptab-terminal".into(), "TERMINAL".into(), self.panel == PanelView::Terminal)
                            .on_click(cx.listener(|wb, _, w, cx| wb.show_terminal(w, cx))),
                    );
                }
                _ => {}
            }
        }
        header = header.child(div().flex_1());
        if self.panel == PanelView::Terminal {
            header = header.children(self.render_terminal_tabs(cx));
        }
        header = header.child(
            div()
                .id("panel-close")
                .p_1()
                .rounded(px(4.))
                .cursor_pointer()
                .hover(|s| s.bg(t.c("toolbar.hoverBackground")))
                .child(icon("close", t.m("panel.iconSize", 14.0), t.c("icon.foreground")))
                .on_click(cx.listener(|wb, _, _, cx| {
                    wb.panel_visible = false;
                    cx.notify();
                })),
        );
        div()
            .relative()
            .flex_none()
            .flex()
            .flex_col()
            .bg(t.bg("panel.background"))
            .border_color(t.c("panel.border"))
            .map(|d| match place {
                PanelPlace::Bottom => d.w_full().h(px(height)).border_t(thickness),
                PanelPlace::Top => d.w_full().h(px(height)).border_b(thickness),
                PanelPlace::Left => d.h_full().w(px(width)).border_r(thickness),
                PanelPlace::Right => d.h_full().w(px(width)).border_l(thickness),
            })
            .child(sash)
            .child(header)
            .child(div().flex_1().min_h_0().child(content))
    }

    /// The status bar: `statusBar.left/center/right` list the items of each zone (branch, sync, problems,
    /// errors, warnings, run, modules, position, indent, encoding, eol, language, lsp, spacer, commands).
    pub(super) fn render_status_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::global(cx);
        let lay = &t.layout;
        let fg = t.c("statusBar.foreground");
        let hover = t.c("statusBarItem.hoverBackground");
        let icon_size = t.m("statusBar.iconSize", 13.0);
        let pad = t.edges("statusBarItem.padding", 8.0).left;
        let radius = t.radius("statusBarItem.radius", 0.0);
        // Each part can be styled on its own: statusBarItem.<part>.background / .foreground (colors) and
        // statusBarItem.<part>.weight (fonts), e.g. "statusBarItem.text:NORMAL.background".
        let item = |part: &str, id: SharedString, icon_name: Option<&str>, text: String| {
            let (bg_key, fg_key) = (format!("statusBarItem.{part}.background"), format!("statusBarItem.{part}.foreground"));
            let color = if t.has(&fg_key) { t.c(&fg_key) } else { fg };
            div()
                .id(id)
                .flex_none()
                .h_full()
                .px(pad)
                .flex()
                .items_center()
                .gap_1()
                .rounded(radius)
                .cursor_pointer()
                .when(t.has(&bg_key), |d| d.bg(t.bg(&bg_key)))
                .text_color(color)
                .font_weight(t.weight(&format!("statusBarItem.{part}.weight"), gpui::FontWeight::NORMAL))
                .hover(move |s| s.bg(hover))
                .children(icon_name.map(|n| icon(n, icon_size, color)))
                .when(!text.is_empty(), |d| d.child(text))
        };
        let problems = self.problems(cx);
        let errors = problems.iter().filter(|(_, d)| d.severity == bf_lsp::Severity::Error).count();
        let warnings = problems.iter().filter(|(_, d)| d.severity == bf_lsp::Severity::Warning).count();
        let running = self.dotnet.as_ref().is_some_and(DotnetProcess::is_running) || self.debug.is_some();
        let debugging = self.debug.as_ref().map(|d| d.status_text());
        let git = self.scm.read(cx).git().is_some().then(|| self.scm.read(cx).status().clone());
        let bom = self.active.and_then(|i| self.tabs.get(i)).is_some_and(|t| t.bom);
        // (position, indent, encoding, eol, language) of the active editor.
        let on_page = self.active.and_then(|i| self.tabs.get(i)).is_some_and(|t| !t.is_document());
        let editor_info = self.active_editor().filter(|_| !on_page).map(|editor| {
            let e = editor.read(cx);
            let caret = e.state.caret();
            let selection = if e.state.has_selection() { format!(" ({} selected)", e.state.selected_text().chars().count()) } else { String::new() };
            let indent = if e.state.insert_spaces { format!("Spaces: {}", e.state.tab_size) } else { format!("Tab Size: {}", e.state.tab_size) };
            let eol = match e.state.buffer.line_ending() {
                bf_text::LineEnding::Lf => "LF",
                bf_text::LineEnding::CrLf => "CRLF",
            };
            let position = t.layout.position_format.replace("{line}", &(caret.line + 1).to_string()).replace("{col}", &(caret.col + 1).to_string()).replace("{lines}", &e.state.buffer.line_count().to_string());
            (format!("{position}{selection}"), indent, if bom { "UTF-8 with BOM" } else { "UTF-8" }, eol, e.language.map_or("Plain Text", |l| l.name()))
        });

        // Module items go to the zone matching their alignment if it lists "modules", else the first zone that does.
        let zones: [&Vec<String>; 3] = [&lay.status_left, &lay.status_center, &lay.status_right];
        let has_modules = |z: usize| zones[z].iter().any(|n| n == "modules");
        let module_zone = |right: bool| {
            let wanted = if right { 2 } else { 0 };
            if has_modules(wanted) { Some(wanted) } else { (0..3).find(|z| has_modules(*z)) }
        };

        let mut out: [Vec<gpui::AnyElement>; 3] = Default::default();
        for (z, names) in zones.iter().enumerate() {
            for (n, name) in names.iter().enumerate() {
                let id = |suffix: &str| SharedString::from(format!("st-{z}-{n}-{suffix}"));
                let el: Vec<gpui::AnyElement> = match name.as_str() {
                    "branch" => git.as_ref().map(|s| {
                        let branch = s.branch.clone().unwrap_or_else(|| if s.detached { "(detached)".into() } else { "—".into() });
                        let dirty = if s.changes.is_empty() { "" } else { "*" };
                        item(name, id("b"), Some("sourceControl"), format!("{branch}{dirty}")).tooltip(tooltip("Checkout branch…")).on_click(cx.listener(|wb, _, w, cx| wb.open_branch_picker(w, cx))).into_any_element()
                    }).into_iter().collect(),
                    "sync" => git.as_ref().filter(|s| s.has_commits).map(|s| {
                        let (text, tip) = match (&s.upstream, s.ahead, s.behind) {
                            (Some(_), 0, 0) => (String::new(), "Synchronize changes (pull, then push)"),
                            (Some(_), a, b) => (format!("{b}↓ {a}↑"), "Synchronize changes (pull, then push)"),
                            (None, _, _) => (String::new(), "Publish branch to origin"),
                        };
                        item(name, id("s"), Some(if s.upstream.is_some() { "sync" } else { "arrowUp" }), text).tooltip(tooltip(tip)).on_click(cx.listener(|wb, _, _, cx| wb.scm.update(cx, |s, cx| s.sync(cx)))).into_any_element()
                    }).into_iter().collect(),
                    "problems" => vec![
                        item(name, id("e"), Some("error"), errors.to_string()).on_click(cx.listener(|wb, _, _, cx| wb.show_panel(PanelView::Problems, cx))).into_any_element(),
                        item(name, id("w"), Some("warning"), warnings.to_string()).on_click(cx.listener(|wb, _, _, cx| wb.show_panel(PanelView::Problems, cx))).into_any_element(),
                    ],
                    "errors" => vec![item(name, id("e"), Some("error"), errors.to_string()).on_click(cx.listener(|wb, _, _, cx| wb.show_panel(PanelView::Problems, cx))).into_any_element()],
                    "warnings" => vec![item(name, id("w"), Some("warning"), warnings.to_string()).on_click(cx.listener(|wb, _, _, cx| wb.show_panel(PanelView::Problems, cx))).into_any_element()],
                    "run" => vec![
                        item(name, id("r"), Some(if running { "stop" } else { "play" }), debugging.clone().unwrap_or_else(|| if running { "Stop".into() } else { "Run".into() }))
                            .tooltip(tooltip(if running { "Stop (Shift+F5)".to_owned() } else { format!("Build and run without debugging ({})", crate::actions::key_label("ctrl-f5")) }))
                            .on_click(cx.listener(move |_, _, w, cx| {
                                if running {
                                    w.dispatch_action(Box::new(Stop), cx)
                                } else {
                                    w.dispatch_action(Box::new(Run), cx)
                                }
                            }))
                            .into_any_element(),
                    ],
                    "update" => self
                        .update_status()
                        .map(|(text, tip)| {
                            item(name, id("up"), Some("arrowDown"), text).tooltip(tooltip(tip)).on_click(cx.listener(|wb, _, w, cx| wb.update_clicked(w, cx))).into_any_element()
                        })
                        .into_iter()
                        .collect(),
                    "launchProfile" => self
                        .status_profile
                        .clone()
                        .map(|profile| {
                            item(name, id("lp"), Some("layers"), profile)
                                .tooltip(tooltip("Launch profile for F5 and Run (click to change)"))
                                .on_click(cx.listener(|wb, _, w, cx| wb.open_launch_profile_picker(w, cx)))
                                .into_any_element()
                        })
                        .into_iter()
                        .collect(),
                    "modules" => self
                        .plugin_status
                        .iter()
                        .enumerate()
                        .filter(|(_, e)| module_zone(e.right) == Some(z) && !e.text.is_empty())
                        .map(|(i, entry)| {
                            let command = entry.command.clone();
                            let mut el = item(name, id(&format!("m{i}")), None, entry.text.clone()).on_click(cx.listener(move |_, _, w, cx| {
                                if let Some(id) = &command {
                                    w.dispatch_action(Box::new(RunModuleCommand { id: id.clone() }), cx);
                                }
                            }));
                            if let Some(tip) = entry.tooltip.clone() {
                                el = el.tooltip(tooltip(tip));
                            }
                            el.into_any_element()
                        })
                        .collect(),
                    "position" | "indent" | "encoding" | "eol" | "language" => {
                        let Some(info) = &editor_info else { continue };
                        let text = match name.as_str() {
                            "position" => info.0.clone(),
                            "indent" => info.1.clone(),
                            "encoding" => info.2.into(),
                            "eol" => info.3.into(),
                            _ => info.4.into(),
                        };
                        let el = item(name, id(name), None, text);
                        vec![match name.as_str() {
                            "position" => el.on_click(cx.listener(|wb, _, w, cx| wb.open_palette(Mode::GoToLine, w, cx))).into_any_element(),
                            "indent" => el.on_click(cx.listener(|_, _, w, cx| w.dispatch_action(Box::new(OpenSettings), cx))).into_any_element(),
                            "eol" => el.on_click(cx.listener(|_, _, w, cx| w.dispatch_action(Box::new(ToggleLineEnding), cx))).into_any_element(),
                            _ => el.into_any_element(),
                        }]
                    }
                    "lsp" => (!self.lsp_status.is_empty())
                        .then(|| item(name, id("c"), None, self.lsp_status.clone()).tooltip(tooltip("C# language server (csharp-ls) — click to restart")).on_click(cx.listener(|wb, _, _, cx| wb.start_lsp(cx))).into_any_element())
                        .into_iter()
                        .collect(),
                    "spacer" => vec![div().flex_1().into_any_element()],
                    "file" => self
                        .active
                        .and_then(|i| self.tabs.get(i))
                        .map(|tab| {
                            let modified = if tab.editor.read(cx).is_modified() { " [+]" } else { "" };
                            item(name, id("f"), None, format!("{}{modified}", tab.title)).into_any_element()
                        })
                        .into_iter()
                        .collect(),
                    other if other.starts_with("text:") => vec![item(name, id("t"), None, other["text:".len()..].to_owned()).into_any_element()],
                    other => command_button(other, cx)
                        .map(|(action, title, icon_name)| {
                            let explicit_icon = crate::layout::command_item(other).and_then(|(_, i)| i).is_some();
                            let el = if explicit_icon { item(name, id("x"), Some(&icon_name), String::new()) } else { item(name, id("x"), None, title.clone()) };
                            el.tooltip(tooltip(title)).on_click(move |_, w, cx| w.dispatch_action(action.boxed_clone(), cx)).into_any_element()
                        })
                        .into_iter()
                        .collect(),
                };
                out[z].extend(el);
            }
        }
        let [left, center, right] = out;
        div()
            .flex_none()
            .h(t.m("statusBar.height", 24.0))
            .w_full()
            .flex()
            .bg(if self.debug.is_some() && t.has("statusBar.debuggingBackground") { t.bg("statusBar.debuggingBackground") } else { t.bg("statusBar.background") })
            .text_color(fg)
            .text_size(t.m("statusBar.fontSize", 12.0))
            .map(|d| {
                let b = px(f32::from(t.edges("statusBar.borderThickness", 1.0).top));
                if lay.workbench.iter().position(|p| p == "statusBar") < lay.workbench.iter().position(|p| p == "main") { d.border_b(b) } else { d.border_t(b) }
            })
            .border_color(t.c("statusBar.border"))
            .child(div().flex_1().min_w_0().h_full().flex().items_center().overflow_hidden().children(left))
            .child(div().flex_none().h_full().flex().items_center().children(center))
            .child(div().flex_1().min_w_0().h_full().flex().items_center().justify_end().overflow_hidden().children(right))
    }
}
