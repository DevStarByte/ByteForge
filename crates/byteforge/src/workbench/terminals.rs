//! The integrated terminal panel: creating, switching and closing terminals.

use super::*;

impl Workbench {
    /// Starts a shell in the opened folder ("terminal.shell" and "terminal.args" in settings, else $SHELL).
    pub(super) fn new_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let shell = crate::terminal::default_shell(self.settings.get_string("terminal.shell"));
        let args: Vec<String> = self.settings.get("terminal.args", Vec::new());
        let cwd = self.workspace.root().or_else(|| std::env::var_os("HOME").map(PathBuf::from));
        let spawned = match crate::terminal::spawn(shell, args, cwd) {
            Ok(spawned) => spawned,
            Err(e) => {
                self.log(format!("Could not start the terminal: {e:#}"));
                self.show_panel(PanelView::Output, cx);
                return;
            }
        };
        let terminal = cx.new(|cx| TerminalView::new(spawned, cx));
        let sub = cx.subscribe_in(&terminal, window, |wb, terminal, event, window, cx| match event {
            TerminalEvent::Exited => {
                // Like VS Code: a terminal whose shell exits goes away.
                if let Some(ix) = wb.terminals.iter().position(|(t, _)| t == terminal) {
                    wb.remove_terminal(ix, window, cx);
                }
            }
            TerminalEvent::TitleChanged => cx.notify(),
            TerminalEvent::OpenFile(path, line, column) => {
                let at = Point::new(line.saturating_sub(1) as usize, column.saturating_sub(1) as usize);
                wb.open_file(path.clone(), Some(at), window, cx);
            }
        });
        self.terminals.push((terminal, sub));
        self.active_terminal = self.terminals.len() - 1;
        self.show_terminal(window, cx);
    }

    /// Shows the terminal panel (starting a terminal if none is open) and focuses it.
    pub(super) fn show_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminals.is_empty() {
            self.new_terminal(window, cx);
            return;
        }
        self.show_panel(PanelView::Terminal, cx);
        if let Some((terminal, _)) = self.terminals.get(self.active_terminal) {
            terminal.read(cx).focus(window);
        }
    }

    /// Ctrl+`: shows and focuses the terminal, or hides the panel when the terminal is focused.
    pub(super) fn toggle_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self.terminals.get(self.active_terminal).is_some_and(|(t, _)| t.focus_handle(cx).contains_focused(window, cx));
        if self.panel_visible && self.panel == PanelView::Terminal && focused {
            self.panel_visible = false;
            match self.active_editor() {
                Some(editor) => editor.update(cx, |e, _| e.focus(window)),
                None => window.focus(&self.focus),
            }
            cx.notify();
        } else {
            self.show_terminal(window, cx);
        }
    }

    /// Lists the installed shells; the pick is saved as "terminal.shell".
    pub(super) fn open_shell_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.settings.get_string("terminal.shell");
        let login = std::env::var("SHELL").unwrap_or_default();
        let mut default = PaletteItem::new("Login shell", PaletteAction::Command(Box::new(SetDefaultShell { shell: String::new() })));
        default.detail = Some(if login.is_empty() { "the system default".into() } else { login.clone() });
        default.hint = current.is_none().then(|| "current".into());
        let mut items = vec![default];
        for shell in crate::terminal::installed_shells() {
            let name = Path::new(&shell).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| shell.clone());
            let mut item = PaletteItem::new(name, PaletteAction::Command(Box::new(SetDefaultShell { shell: shell.clone() })));
            item.detail = Some(shell.clone());
            item.hint = (current.as_deref() == Some(shell.as_str())).then(|| "current".into());
            item.search = Some(shell);
            items.push(item);
        }
        self.menu_open = None;
        let palette = cx.new(|cx| Palette::new(Mode::Pick, items, "Select the default shell for new terminals", "", window, cx));
        let sub = cx.subscribe_in(&palette, window, Self::on_palette_event);
        palette.update(cx, |p, cx| p.focus_input(window, cx));
        self.palette = Some((palette, sub));
        cx.notify();
    }

    pub(super) fn kill_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.terminals.is_empty() {
            self.remove_terminal(self.active_terminal, window, cx);
        }
    }

    pub(super) fn remove_terminal(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.terminals.len() {
            return;
        }
        drop(self.terminals.remove(ix));
        self.active_terminal = self.active_terminal.min(self.terminals.len().saturating_sub(1));
        if self.terminals.is_empty() && self.panel == PanelView::Terminal {
            self.panel_visible = false;
            match self.active_editor() {
                Some(editor) => editor.update(cx, |e, _| e.focus(window)),
                None => window.focus(&self.focus),
            }
        } else if self.panel == PanelView::Terminal
            && let Some((terminal, _)) = self.terminals.get(self.active_terminal)
        {
            terminal.read(cx).focus(window);
        }
        cx.notify();
    }

    /// The terminal switcher in the panel header: one chip per terminal, then new and kill buttons.
    pub(super) fn render_terminal_tabs(&self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let t = Theme::global(cx);
        let hover = t.c("toolbar.hoverBackground");
        let icon_size = t.m("panel.iconSize", 14.0);
        let button = |id: &'static str, name: &'static str, tip: &'static str| {
            div().id(id).p_1().rounded(px(4.)).cursor_pointer().hover(move |s| s.bg(hover)).tooltip(tooltip(tip)).child(icon(name, icon_size, t.c("icon.foreground")))
        };
        let mut out: Vec<gpui::AnyElement> = Vec::new();
        if self.terminals.len() > 1 {
            for (i, (terminal, _)) in self.terminals.iter().enumerate() {
                let active = i == self.active_terminal;
                out.push(
                    div()
                        .id(SharedString::from(format!("term-{i}")))
                        .mx_1()
                        .px_2()
                        .py(px(2.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .text_size(px(11.))
                        .when(active, |d| d.bg(t.c("list.activeSelectionBackground")).text_color(t.c("list.activeSelectionForeground")))
                        .when(!active, |d| d.text_color(t.c("panelTitle.inactiveForeground")).hover(move |s| s.bg(hover)))
                        .child(format!("{}: {}", i + 1, short(terminal.read(cx).display_title(), 24)))
                        .on_click(cx.listener(move |wb, _, w, cx| {
                            wb.active_terminal = i;
                            wb.show_terminal(w, cx);
                        }))
                        .into_any_element(),
                );
            }
        }
        out.push(button("term-new", "add", "New Terminal (Ctrl+Shift+`)").on_click(cx.listener(|wb, _, w, cx| wb.new_terminal(w, cx))).into_any_element());
        out.push(button("term-shell", "terminal", "Select Default Shell…").on_click(cx.listener(|wb, _, w, cx| wb.open_shell_picker(w, cx))).into_any_element());
        out.push(button("term-kill", "trash", "Kill Terminal").on_click(cx.listener(|wb, _, w, cx| wb.kill_terminal(w, cx))).into_any_element());
        out
    }

    // ───────────── modules ─────────────
}
