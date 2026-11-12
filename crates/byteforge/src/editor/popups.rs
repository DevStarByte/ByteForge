//! Code-action menu, signature help and hover popups.

use super::*;

impl EditorView {
    pub(super) fn render_action_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let menu = self.action_menu.as_ref()?;
        let t = Theme::global(cx);
        let (x, y) = self.caret_screen_pos(menu.at);
        let lh = self.layout.line_height;
        let row_h = t.n("list.itemHeight", 22.0).max(lh * 0.9);
        let muted = t.c("foreground.muted");
        let quick_fix = menu.mode == refactor::MenuMode::QuickFix;
        let message = |text: String| div().px_2().h(px(row_h)).flex().items_center().text_color(muted).child(text).into_any_element();
        let rows: Vec<gpui::AnyElement> = match &menu.items {
            None if quick_fix => vec![message("Looking for fixes…".into())],
            None => vec![message("Looking for refactorings…".into())],
            Some(items) if items.is_empty() => {
                let default = if quick_fix { "No fixes or refactorings available here" } else { "No refactorings available here" };
                vec![message(menu.empty.clone().unwrap_or_else(|| default.into()))]
            }
            Some(items) => items
                .iter()
                .enumerate()
                .map(|(i, action)| {
                    let group = refactor::group_of(action);
                    let (glyph, color) = match group {
                        "Fix" => ("build", t.c_or("editorLightBulb.foreground", 0xffcc00ff)),
                        "Rename" => ("pencil", t.c_or("editorLightBulbAutoFix.foreground", 0x75beffff)),
                        "Merge" => ("diff", t.c_or("editorLightBulb.foreground", 0xffcc00ff)),
                        _ => ("puzzle", t.c_or("editorLightBulbAutoFix.foreground", 0x75beffff)),
                    };
                    let selected = i == menu.selected;
                    let disabled = action.disabled.is_some();
                    div()
                        .id(gpui::SharedString::from(format!("action-{i}")))
                        .h(px(row_h))
                        .px_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded(t.radius("list.radius", 0.0))
                        .cursor_pointer()
                        .when(selected, |d| d.bg(t.bg("list.activeSelectionBackground")).text_color(t.c("list.activeSelectionForeground")))
                        .when(!selected, |d| d.hover(|s| s.bg(t.c("list.hoverBackground"))))
                        .when(disabled, |d| d.text_color(muted))
                        .child(icon(glyph, px(14.), color))
                        .child(div().flex_1().overflow_hidden().whitespace_nowrap().child(action.display_title()))
                        .when(action.preferred, |d| d.child(div().text_size(px(11.)).text_color(muted).child("preferred")))
                        .child(div().text_size(px(11.)).text_color(muted).child(group))
                        .when_some(action.disabled.clone(), |d, reason| d.tooltip(crate::workbench::tooltip(reason)))
                        .on_mouse_down(MouseButton::Left, cx.listener(move |v, _, _, cx| {
                            cx.stop_propagation();
                            v.commit_action(Some(i), cx);
                        }))
                        .into_any_element()
                })
                .collect(),
        };
        Some(
            div()
                .id("action-menu")
                .absolute()
                .left(px(x.max(4.0)))
                .top(px(y + lh))
                .min_w(px(280.))
                .max_w(px(640.))
                .max_h(px(320.))
                .overflow_y_scroll()
                .bg(t.bg("editorWidget.background"))
                .border_color(t.c("editorWidget.border"))
                .border_1()
                .rounded(t.radius("widget.radius", 6.0))
                .shadow(t.shadow("widget.shadow"))
                .p(px(2.))
                .text_size(t.m("ui.size", 13.0))
                .font_family(t.ui_font.clone())
                .text_color(t.c("editor.foreground"))
                .children(rows),
        )
    }

    /// Parameter hints above the caret line: the signature with the current parameter in bold and
    /// underlined, its overload number (click the arrows to switch) and documentation.
    pub(super) fn render_signature(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let state = self.signature.as_ref()?;
        // The completion list sits below the line; hints go above it, unless there's no room.
        let t = Theme::global(cx);
        let signature = state.help.signatures.get(state.selected)?;
        let (x, y) = self.caret_screen_pos(self.state.caret());
        let lh = self.layout.line_height;
        let above = y > 70.0;
        let fg = t.c("editorHoverWidget.foreground");
        let highlight = t.c_or("editorHoverWidget.highlightForeground", 0x4fc1ffff);
        let mut label = div().flex().flex_wrap().font_family(t.editor_font.clone());
        let active = signature.active_parameter.and_then(|i| signature.parameters.get(i)).cloned();
        match active {
            Some(range) => {
                label = label
                    .child(signature.label[..range.start].to_owned())
                    .child(div().text_color(highlight).font_weight(gpui::FontWeight::BOLD).underline().child(signature.label[range.clone()].to_owned()))
                    .child(signature.label[range.end..].to_owned());
            }
            None => label = label.child(signature.label.clone()),
        }
        let count = state.help.signatures.len();
        let arrow = |id: &'static str, name: &'static str, delta: isize| {
            div()
                .id(id)
                .rounded(px(3.))
                .cursor_pointer()
                .hover(|s| s.bg(t.c("toolbar.hoverBackground")))
                .child(icon(name, px(14.), fg))
                .on_mouse_down(MouseButton::Left, cx.listener(move |v, _, _, cx| {
                    cx.stop_propagation();
                    v.cycle_signature(delta, cx);
                }))
        };
        let header = div()
            .flex()
            .items_start()
            .gap_2()
            .when(count > 1, |d| {
                d.child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .text_size(px(11.))
                        .text_color(t.c("foreground.muted"))
                        .child(arrow("sig-prev", "chevronUp", -1))
                        .child(format!("{}/{count}", state.selected + 1))
                        .child(arrow("sig-next", "chevronDown", 1)),
                )
            })
            .child(div().flex_1().child(label));
        let doc = signature.documentation.as_deref().map(|d| d.split("\n\n").next().unwrap_or(d).trim().to_owned()).filter(|d| !d.is_empty());
        Some(
            div()
                .id("signature-help")
                .absolute()
                .left(px(x.max(4.0)))
                .map(|d| if above { d.bottom(px(self.layout.height - y + 2.0)) } else { d.top(px(y + lh + 2.0)) })
                .max_w(px(640.))
                .bg(t.bg("editorHoverWidget.background"))
                .text_color(fg)
                .border_color(t.c("editorHoverWidget.border"))
                .border_1()
                .rounded(t.radius("widget.radius", 6.0))
                .shadow(t.shadow("widget.shadow"))
                .px_2()
                .py_1()
                .text_size(t.m("ui.size", 13.0))
                .font_family(t.ui_font.clone())
                .cursor(gpui::CursorStyle::Arrow)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .flex()
                .flex_col()
                .gap_1()
                .child(header)
                .when_some(doc, |d, doc| d.child(div().text_size(px(12.)).text_color(t.c("foreground.muted")).child(crate::markdown::render(&doc, cx)))),
        )
    }

    pub(super) fn render_hover(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let t = Theme::global(cx);
        let hover = self.hover.as_ref()?;
        let problems: Vec<(&'static str, &'static str, String)> = self
            .diagnostics_in(hover.range)
            .map(|d| {
                let (name, key) = match d.severity {
                    Severity::Error => ("error", "editorError.foreground"),
                    Severity::Warning => ("warning", "editorWarning.foreground"),
                    _ => ("info", "editorInfo.foreground"),
                };
                let text = match &d.code {
                    Some(code) => format!("{} ({code})", d.message),
                    None => d.message.clone(),
                };
                (name, key, text)
            })
            .collect();
        let has_text = !hover.text.trim().is_empty();
        if problems.is_empty() && !has_text {
            return None;
        }
        let (x, y) = self.caret_screen_pos(hover.range.0);
        let lh = self.layout.line_height;
        // Below the line in the upper half of the editor, above it in the lower half, never covering it.
        let below = y + lh / 2.0 < self.layout.height / 2.0;
        let max_h = if below { self.layout.height - y - lh - 8.0 } else { y - 8.0 }.clamp(80.0, 360.0);
        let mut body = div().flex().flex_col().gap(px(6.));
        let has_problems = !problems.is_empty();
        for (name, key, text) in problems {
            body = body.child(div().flex().gap_2().child(icon(name, px(14.), t.c(key)).mt(px(2.))).child(div().flex_1().child(text)));
        }
        if has_problems && self.lsp.is_some() {
            body = body.child(div().text_size(px(11.)).text_color(t.c("foreground.muted")).child(format!("{} for quick fixes", crate::actions::key_label("ctrl-."))));
        }
        if has_text {
            if self.diagnostics_in(hover.range).next().is_some() {
                body = body.child(div().h(px(1.)).w_full().bg(t.c("editorHoverWidget.border")));
            }
            body = body.child(crate::markdown::render(&hover.text, cx));
        }
        Some(
            div()
                .id("hover-popup")
                .absolute()
                .left(px(x.max(4.0)))
                .map(|d| if below { d.top(px(y + lh + 2.0)) } else { d.bottom(px(self.layout.height - y + 2.0)) })
                .max_w(px(600.))
                .max_h(px(max_h))
                .overflow_y_scroll()
                .bg(t.bg("editorHoverWidget.background"))
                .text_color(t.c("editorHoverWidget.foreground"))
                .border_color(t.c("editorHoverWidget.border"))
                .border_1()
                .rounded(t.radius("widget.radius", 6.0))
                .shadow(t.shadow("widget.shadow"))
                .p_2()
                .text_size(t.m("ui.size", 13.0))
                .font_family(t.ui_font.clone())
                .cursor(gpui::CursorStyle::Arrow)
                // Reading the popup: keep the editor from scrolling or selecting underneath.
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child({
                    let store = self.hover_bounds.clone();
                    gpui::canvas(move |bounds, _, _| store.set(Some(bounds)), |_, _, _, _| {}).absolute().inset_0()
                })
                .child(body),
        )
    }
}

/// A context menu entry: label, action and key hint (`None` in the list is a separator).
type MenuEntry = (String, Box<dyn gpui::Action>, String);

impl EditorView {
    /// The right-click menu: navigation and refactoring for C# (with the language server), then the
    /// clipboard and formatting.
    pub(super) fn render_context_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let menu = self.context_menu.as_ref()?;
        let t = Theme::global(cx);
        let commands = cx.global::<Commands>();
        let mut entries: Vec<Option<MenuEntry>> = Vec::new();
        let command = |entries: &mut Vec<_>, id: &str| {
            if let Some(cmd) = commands.get(id) {
                entries.push(Some((cmd.title.clone(), cmd.action.boxed_clone(), crate::workbench::default_key(id))));
            }
        };
        if !self.read_only && !self.conflict_entries().is_empty() {
            for id in ["merge.acceptCurrent", "merge.acceptIncoming", "merge.acceptBoth", "merge.editor"] {
                command(&mut entries, id);
            }
            entries.push(None);
        }
        let code = self.lsp.is_some() && !self.read_only;
        if code {
            for id in ["edit.goToDefinition", "edit.goToImplementation", "edit.findReferences"] {
                command(&mut entries, id);
            }
            entries.push(None);
            for id in ["edit.rename", "refactor.menu", "edit.quickFix"] {
                command(&mut entries, id);
            }
            entries.push(None);
            for id in ["refactor.extractMethod", "refactor.introduceVariable", "refactor.inline", "refactor.removeAndSortUsings"] {
                command(&mut entries, id);
            }
            entries.push(None);
        }
        if self.glyph_margin && !self.read_only {
            for id in ["debug.toggleBreakpoint", "debug.conditionalBreakpoint", "debug.logpoint", "debug.hitCount"] {
                command(&mut entries, id);
            }
            entries.push(None);
        }
        if self.blame.repo.is_some() && self.path.is_some() {
            command(&mut entries, "git.showLineCommit");
            command(&mut entries, "git.fileHistory");
            entries.push(None);
        }
        let key = crate::actions::key_label;
        if !self.read_only {
            entries.push(Some(("Cut".into(), Box::new(Cut), key("ctrl-x"))));
        }
        entries.push(Some(("Copy".into(), Box::new(Copy), key("ctrl-c"))));
        if !self.read_only {
            entries.push(Some(("Paste".into(), Box::new(Paste), key("ctrl-v"))));
            entries.push(None);
            command(&mut entries, "edit.toggleComment");
            command(&mut entries, "edit.formatDocument");
        }
        let mut panel = crate::menu::panel(&t, 240.).on_mouse_down_out(cx.listener(|v, _, _, cx| {
            v.context_menu = None;
            cx.notify();
        }));
        for (ix, entry) in entries.into_iter().enumerate() {
            panel = match entry {
                None => panel.child(crate::menu::separator(&t)),
                Some((label, action, hint)) => panel.child(
                    crate::menu::item(&t, gpui::SharedString::from(format!("editor-menu-{ix}")), label, Some(hint.into()))
                        // Keep the press from reaching the editor below (it would move the caret).
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |v, _, window, cx| {
                            v.context_menu = None;
                            window.focus(&v.focus);
                            window.dispatch_action(action.boxed_clone(), cx);
                            cx.notify();
                        })),
                ),
            };
        }
        Some(gpui::deferred(gpui::anchored().position(menu.position).snap_to_window().child(panel)).with_priority(1))
    }
}
