//! Module processes, formatters and plugin messages.

use super::*;

impl Workbench {
    pub(super) fn start_plugins(&mut self, _event_tx: flume::Sender<AppEvent>, cx: &mut Context<Self>) {
        let disabled: Vec<String> = self.settings.get("modules.disabled", Vec::new());
        self.preinstall_modules();
        self.modules = discover(&self.paths.user_modules());
        let (tx, rx) = flume::unbounded::<PluginMessage>();
        self.plugin_tx = Some(tx);
        for manifest in self.modules.clone() {
            if disabled.contains(&manifest.id) {
                self.log(format!("Module {} ({}) is disabled.", manifest.name, manifest.id));
                continue;
            }
            self.start_module(manifest);
        }
        self.refresh_module_contributions(cx);
        self._tasks.push(cx.spawn(async move |this, cx| {
            while let Ok(message) = rx.recv_async().await {
                if this.update(cx, |wb, cx| wb.on_plugin_message(message, cx)).is_err() {
                    break;
                }
            }
        }));
    }

    pub(super) fn start_module(&mut self, manifest: ModuleManifest) {
        if manifest.command.trim().is_empty() {
            return; // only declares things (file icons), nothing to run
        }
        let Some(tx) = self.plugin_tx.clone() else { return };
        match PluginHandle::start(manifest.clone(), tx) {
            Ok(handle) => {
                handle.activate(self.workspace.root().as_deref(), &self.settings.all());
                // A restarted module learns again that its view is visible.
                if self.module_view_shown.as_ref().is_some_and(|(m, _)| *m == manifest.id) {
                    self.module_view_shown = None;
                }
                handle.event("editorChanged", self.active_path().as_deref());
                self.log(format!("Module '{}' started", manifest.name));
                self.plugins.push(handle);
            }
            Err(e) => self.log(format!("Module '{}' failed to start: {e}", manifest.name)),
        }
    }

    /// Stops a module and drops everything it contributed (formatters, commands, status items).
    pub(super) fn stop_module(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(i) = self.plugins.iter().position(|p| p.manifest.id == id) {
            self.plugins.remove(i).stop();
        }
        self.formatters.retain(|f| f.module != id);
        self.plugin_status.retain(|s| s.module != id);
        let commands: Vec<String> = self.plugin_commands.iter().filter(|(_, m)| *m == id).map(|(c, _)| c.clone()).collect();
        for command in &commands {
            self.plugin_commands.remove(command);
        }
        cx.global_mut::<Commands>().list.retain(|c| !commands.contains(&c.id));
        cx.notify();
    }

    // ───────────── formatting ─────────────

    /// The formatter for a file: "editor.defaultFormatter" if it handles the file, else the first module
    /// formatter that does, else the C# language server. Returns (id, name).
    pub(super) fn formatter_for(&self, editor: &EditorView) -> Option<(String, String)> {
        let ext = editor.path.as_deref().and_then(|p| p.extension()).map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let handles = |f: &&Formatter| f.extensions.contains(&ext);
        let lsp = (editor.language == Some(bf_syntax::Language::CSharp) && self.lsp.is_some()).then(|| ("csharp-ls".to_owned(), "csharp-ls".to_owned()));
        let preferred = self.settings.get_string("editor.defaultFormatter");
        if let Some(preferred) = preferred.as_deref() {
            if preferred == "csharp-ls" && lsp.is_some() {
                return lsp;
            }
            if let Some(f) = self.formatters.iter().filter(handles).find(|f| f.id == preferred) {
                return Some((f.id.clone(), f.name.clone()));
            }
        }
        self.formatters.iter().find(handles).map(|f| (f.id.clone(), f.name.clone())).or(lsp)
    }

    /// Formats a tab with its formatter; `save_after` saves it once formatted (format on save). Returns
    /// false when no formatter handles the file.
    pub(super) fn format_tab(&mut self, ix: usize, formatter: Option<String>, save_after: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(tab) = self.tabs.get(ix) else { return false };
        let editor = tab.editor.clone();
        let (id, name) = match formatter {
            Some(id) => {
                let name = self.formatters.iter().find(|f| f.id == id).map_or_else(|| id.clone(), |f| f.name.clone());
                (id, name)
            }
            None => match self.formatter_for(editor.read(cx)) {
                Some(found) => found,
                None => return false,
            },
        };
        let (path, text, version, tab_size, spaces) = {
            let e = editor.read(cx);
            (e.path.clone(), e.state.buffer.text(), e.state.buffer.version(), e.state.tab_size, e.state.insert_spaces)
        };
        let Some(path) = path else { return false };
        if id == "csharp-ls" {
            let Some(lsp) = self.lsp.clone() else { return false };
            let rx = lsp.formatting(&path, tab_size as u32, spaces);
            let weak = editor.downgrade();
            cx.spawn_in(window, async move |this, cx| {
                let result = rx.recv_async().await;
                let _ = this.update_in(cx, |wb, window, cx| {
                    match result {
                        Ok(Ok(edits)) => {
                            if let Some(editor) = weak.upgrade().filter(|e| e.read(cx).state.buffer.version() == version) {
                                editor.update(cx, |e, cx| e.apply_text_edits(&edits, cx));
                            }
                        }
                        Ok(Err(e)) => wb.log(format!("csharp-ls could not format {}: {e:#}", path.display())),
                        Err(_) => {}
                    }
                    if save_after {
                        wb.save_editor(&weak, window, cx);
                    }
                });
            })
            .detach();
            return true;
        }
        let Some(formatter) = self.formatters.iter().find(|f| f.id == id) else { return false };
        let Some(plugin) = self.plugins.iter().find(|p| p.manifest.id == formatter.module) else { return false };
        let request = self.next_format_request;
        self.next_format_request += 1;
        plugin.format(request, &path, &text);
        self.format_requests.insert(request, PendingFormat { editor: editor.downgrade(), version, save_after, name });
        // A module that never answers mustn't keep a save from happening.
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(15)).await;
            let _ = this.update_in(cx, |wb, window, cx| {
                if let Some(pending) = wb.format_requests.remove(&request) {
                    wb.log(format!("{} did not answer in time; the file was not formatted.", pending.name));
                    if pending.save_after {
                        wb.save_editor(&pending.editor, window, cx);
                    }
                }
            });
        })
        .detach();
        true
    }

    pub(super) fn on_format_result(&mut self, request: u64, result: Result<String, String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pending) = self.format_requests.remove(&request) else { return };
        if let Some(editor) = pending.editor.upgrade() {
            match result {
                Ok(text) if editor.read(cx).state.buffer.version() == pending.version => editor.update(cx, |e, cx| e.replace_text_minimal(&text, cx)),
                Ok(_) => self.log(format!("The file changed while {} was formatting it; run Format Document again.", pending.name)),
                Err(error) => {
                    self.log(format!("{}: {error}", pending.name));
                    if !pending.save_after {
                        self.show_panel(PanelView::Output, cx);
                    }
                }
            }
        }
        if pending.save_after {
            self.save_editor(&pending.editor, window, cx);
        }
    }

    /// Saves the tab showing `editor`, if it's still open.
    pub(super) fn save_editor(&mut self, editor: &gpui::WeakEntity<EditorView>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.tabs.iter().position(|t| t.editor.downgrade() == *editor) {
            self.save_tab(ix, window, cx);
        }
    }

    /// Ctrl+S: formats first when "editor.formatOnSave" is on and a formatter handles the file.
    pub(super) fn save_formatted(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let on: bool = self.settings.get("editor.formatOnSave", false);
        let has_path = self.tabs.get(ix).is_some_and(|t| t.path.is_some());
        if !(on && has_path && self.format_tab(ix, None, true, window, cx)) {
            self.save_tab(ix, window, cx);
        }
    }

    pub(super) fn open_formatter_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor() else { return };
        let (ext, csharp) = {
            let e = editor.read(cx);
            (e.path.as_deref().and_then(|p| p.extension()).map(|x| x.to_string_lossy().to_ascii_lowercase()).unwrap_or_default(), e.language == Some(bf_syntax::Language::CSharp))
        };
        let current = self.formatter_for(editor.read(cx)).map(|(id, _)| id);
        let mut items: Vec<PaletteItem> = self
            .formatters
            .iter()
            .filter(|f| f.extensions.contains(&ext))
            .map(|f| {
                let mut item = PaletteItem::new(f.name.clone(), PaletteAction::Command(Box::new(UseFormatter { id: f.id.clone() })));
                item.detail = Some(format!("module {}", f.module));
                item.hint = (current.as_deref() == Some(f.id.as_str())).then(|| "default".into());
                item
            })
            .collect();
        if csharp && self.lsp.is_some() {
            let mut item = PaletteItem::new("csharp-ls (Roslyn formatter)", PaletteAction::Command(Box::new(UseFormatter { id: "csharp-ls".into() })));
            item.hint = (current.as_deref() == Some("csharp-ls")).then(|| "default".into());
            items.push(item);
        }
        if items.is_empty() {
            self.log(format!("No formatter handles .{ext} files."));
            self.show_panel(PanelView::Output, cx);
            return;
        }
        self.menu_open = None;
        let palette = cx.new(|cx| Palette::new(Mode::Pick, items, "Select the formatter (it becomes the default)", "", window, cx));
        let sub = cx.subscribe_in(&palette, window, Self::on_palette_event);
        palette.update(cx, |p, cx| p.focus_input(window, cx));
        self.palette = Some((palette, sub));
        cx.notify();
    }

    /// Runs `f` with the active window once the current update is done. Module messages are handled while the
    /// workbench is already being updated, so updating it again right away (to get a `Window`) would panic.
    pub(super) fn with_window(cx: &mut Context<Self>, f: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static) {
        let entity = cx.entity();
        cx.defer(move |cx| {
            if let Some(window) = cx.active_window() {
                let _ = window.update(cx, |_, window, cx| entity.update(cx, |wb, cx| f(wb, window, cx)));
            }
        });
    }

    pub(super) fn on_plugin_message(&mut self, message: PluginMessage, cx: &mut Context<Self>) {
        match message {
            PluginMessage::RegisterCommand { module, id, title, category, keybinding } => {
                let action = RunModuleCommand { id: id.clone() };
                if let Some(key) = keybinding
                    && let Ok(b) = KeyBinding::load(&key, Box::new(action.clone()), None, false, None, cx.keyboard_mapper().as_ref())
                {
                    cx.bind_keys([b]);
                }
                let category: &'static str = Box::leak(category.unwrap_or_else(|| "Module".into()).into_boxed_str());
                cx.global_mut::<Commands>().list.push(CommandInfo { id: id.clone(), title, category, action: Box::new(action) });
                self.plugin_commands.insert(id, module);
            }
            PluginMessage::Status { module, id, text, tooltip, command, right } => {
                match self.plugin_status.iter_mut().find(|s| s.id == id) {
                    Some(s) => {
                        s.text = text;
                        s.tooltip = tooltip;
                        s.command = command;
                        s.right = right;
                    }
                    None => self.plugin_status.push(StatusEntry { module, id, text, tooltip, command, right }),
                }
            }
            PluginMessage::Log { module, text } => self.log(format!("[{module}] {text}")),
            PluginMessage::OpenFile { path, line, column } => {
                let point = line.map(|l| Point::new(l, column.unwrap_or(0)));
                Self::with_window(cx, move |wb, window, cx| wb.open_file(path, point, window, cx));
            }
            PluginMessage::InsertText { text } => {
                if let Some(editor) = self.active_editor() {
                    editor.update(cx, |e, cx| e.insert_text(&text, cx));
                }
            }
            PluginMessage::SetTheme { name } => {
                self.settings.set("workbench.theme", json!(name));
                Self::with_window(cx, move |wb, window, cx| wb.apply_theme(&name, window, cx));
            }
            PluginMessage::ShowView { id } => match id.as_str() {
                "output" => self.show_panel(PanelView::Output, cx),
                "problems" => self.show_panel(PanelView::Problems, cx),
                "explorer" => self.show_side(SideView::Explorer, cx),
                "search" => self.show_side(SideView::Search, cx),
                other if self.module_views.iter().any(|v| v.id == other) => self.show_module_view(other, cx),
                other => self.show_side(side_view_named(other).unwrap_or(SideView::Modules), cx),
            },
            PluginMessage::SetTree { module, tree } => self.set_module_tree(module, tree),
            PluginMessage::RunInTerminal { title, command, cwd, save_all } => Self::with_window(cx, move |wb, window, cx| {
                if save_all {
                    wb.save_all(window, cx);
                }
                wb.run_in_terminal(title, command, cwd, window, cx);
            }),
            PluginMessage::QuickPick { module, request, placeholder, items } => {
                Self::with_window(cx, move |wb, window, cx| wb.module_quick_pick(module, request, placeholder, items, window, cx));
            }
            PluginMessage::Prompt { module, prompt } => {
                Self::with_window(cx, move |wb, window, cx| wb.module_prompt(module, prompt, window, cx));
            }
            PluginMessage::RegisterFormatter { module, id, name, extensions } => {
                self.formatters.retain(|f| f.id != id);
                self.formatters.push(Formatter { module, id, name, extensions });
            }
            PluginMessage::FormatResult { request, result, .. } => {
                Self::with_window(cx, move |wb, window, cx| wb.on_format_result(request, result, window, cx));
            }
            PluginMessage::Exited { module, reason } => {
                self.formatters.retain(|f| f.module != module);
                self.module_views_stopped(&module);
                self.log(format!("Module {module} stopped: {reason}"));
            }
        }
        cx.notify();
    }

    // ───────────── views ─────────────
}
