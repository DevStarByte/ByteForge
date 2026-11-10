//! Language server startup, code actions and workspace edits.

use super::*;

impl Workbench {
    pub(super) fn start_lsp(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.root() else { return };
        if let Some(old) = self.lsp.take() {
            for tab in &self.tabs {
                tab.editor.update(cx, |e, _| e.detach_lsp());
            }
            old.shutdown();
        }
        let has_csharp = self.workspace.files().iter().any(|f| matches!(f.extension().and_then(|e| e.to_str()), Some("cs" | "csproj" | "sln" | "slnx")));
        if !has_csharp {
            self.lsp_status = String::new();
            return;
        }
        let command = self.settings.get_string("csharp.languageServer").or_else(bf_lsp::csharp_ls_command);
        let Some(command) = command else {
            self.lsp_status = "C#: no server".into();
            self.log("csharp-ls not found. Install it with: dotnet tool install -g csharp-ls (or set \"csharp.languageServer\").");
            return;
        };
        match LspClient::start(ServerConfig { command, args: Vec::new(), root, language_id: "csharp".into() }) {
            Ok((client, events)) => {
                self.lsp_status = "C#: starting…".into();
                for tab in &self.tabs {
                    let client = client.clone();
                    tab.editor.update(cx, |e, cx| {
                        if e.language == Some(bf_syntax::Language::CSharp) {
                            e.attach_lsp(client, cx);
                        }
                    });
                }
                self.lsp = Some(client);
                self.lsp_task = Some(cx.spawn(async move |this, cx| {
                    while let Ok(event) = events.recv_async().await {
                        let ok = this
                            .update(cx, |wb, cx| {
                                match event {
                                    LspEvent::Ready => wb.lsp_status = "C#: loading…".into(),
                                    LspEvent::Progress(text) => {
                                        wb.lsp_status = if text.contains("Finished") { "C#: ready".into() } else { format!("C#: {}", short(&text, 48)) };
                                    }
                                    LspEvent::Diagnostics { path, diagnostics, .. } => {
                                        if let Some(tab) = wb.tabs.iter().find(|t| t.path.as_deref().is_some_and(|p| bf_core::paths::same_path(p, &path))) {
                                            tab.editor.update(cx, |e, cx| e.set_lsp_diagnostics(diagnostics, cx));
                                        }
                                    }
                                    LspEvent::Log(text) => {
                                        if text.to_lowercase().contains("error") {
                                            wb.log(format!("[csharp-ls] {text}"));
                                        }
                                    }
                                    LspEvent::ApplyEdit(edit) => wb.apply_workspace_edit(&edit, None, cx),
                                    LspEvent::Exited(reason) => {
                                        wb.lsp_status = "C#: stopped".into();
                                        wb.log(format!("C# language server exited: {reason}"));
                                    }
                                }
                                cx.notify();
                            })
                            .is_ok();
                        if !ok {
                            break;
                        }
                    }
                }));
            }
            Err(e) => {
                self.lsp_status = "C#: failed".into();
                self.log(format!("Could not start the C# language server: {e}"));
            }
        }
        cx.notify();
    }

    // ───────────── code actions ─────────────

    /// Applies a quick fix or refactoring: resolves its edit if the server computes it lazily, applies the
    /// edit, then runs its command.
    pub(super) fn apply_code_action(&mut self, action: bf_lsp::CodeAction, window: &mut Window, cx: &mut Context<Self>) {
        let Some(lsp) = self.lsp.clone() else { return };
        // Lazily computed actions carry `data` instead of an edit.
        let resolve = action.edit.is_none() && (action.needs_resolve() || action.raw.get("data").is_some_and(|d| !d.is_null()));
        if !resolve {
            self.finish_code_action(action, window, cx);
            return;
        }
        let rx = lsp.resolve_code_action(&action);
        cx.spawn_in(window, async move |this, cx| {
            let result = rx.recv_async().await;
            let _ = this.update_in(cx, |wb, window, cx| match result {
                Ok(Ok(resolved)) => wb.finish_code_action(resolved, window, cx),
                Ok(Err(e)) => wb.log(format!("Could not apply \"{}\": {e:#}", action.title)),
                Err(_) => {}
            });
        })
        .detach();
    }

    pub(super) fn finish_code_action(&mut self, action: bf_lsp::CodeAction, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(edit) = &action.edit {
            self.apply_workspace_edit(edit, Some(window), cx);
        }
        if let (Some(command), Some(lsp)) = (&action.command, &self.lsp) {
            let _ = lsp.execute_command(command);
        }
        if action.edit.is_none() && action.command.is_none() {
            self.log(format!("\"{}\" made no changes.", action.title));
        }
    }

    /// Applies a language-server edit: open files change in their editor (one undo step each). With a
    /// window, other files are opened first so the change can be reviewed and saved; without one they are
    /// changed on disk.
    pub(super) fn apply_workspace_edit(&mut self, edit: &bf_lsp::WorkspaceEdit, mut window: Option<&mut Window>, cx: &mut Context<Self>) {
        for path in &edit.creates {
            if !path.exists() {
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                if let Err(e) = std::fs::write(path, "") {
                    self.log(format!("Could not create {}: {e}", path.display()));
                }
            }
        }
        let active = self.active;
        for (path, edits) in &edit.changes {
            let path = normalize_path(path);
            let find = |wb: &Self| wb.tabs.iter().find(|t| t.path.as_deref().is_some_and(|p| bf_core::paths::same_path(p, &path))).map(|t| t.editor.clone());
            let mut editor = find(self);
            if editor.is_none()
                && let Some(window) = window.as_deref_mut()
            {
                self.open_file(path.clone(), None, window, cx);
                editor = find(self);
            }
            match editor {
                Some(editor) => editor.update(cx, |e, cx| e.apply_text_edits(edits, cx)),
                None => {
                    let result = std::fs::read_to_string(&path).and_then(|text| {
                        let mut buffer = bf_text::Buffer::new(&text);
                        let mut points: Vec<_> = edits
                            .iter()
                            .map(|e| {
                                let start = buffer.utf16_to_point(e.range.start.line as usize, e.range.start.character as usize);
                                let end = buffer.utf16_to_point(e.range.end.line as usize, e.range.end.character as usize);
                                (start, end, e.new_text.replace("\r\n", "\n"))
                            })
                            .collect();
                        points.sort_by_key(|p| std::cmp::Reverse(p.0));
                        for (start, end, text) in points {
                            buffer.replace(start, end, &text);
                        }
                        std::fs::write(&path, buffer.text_for_save())
                    });
                    if let Err(e) = result {
                        self.log(format!("Could not change {}: {e}", path.display()));
                    }
                }
            }
        }
        // Opening files to edit them shouldn't move you away from where you were.
        if let (Some(ix), Some(window)) = (active, window) {
            self.activate(ix, window, cx);
        }
        cx.notify();
    }

    // ───────────── dotnet ─────────────
}
