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
        let config = match self.settings.get_string("csharp.server").as_deref() {
            Some("roslyn") => match self.roslyn_server_config(&root, cx) {
                Some(config) => config,
                None => return,
            },
            _ => {
                let command = self.settings.get_string("csharp.languageServer").or_else(bf_lsp::csharp_ls_command);
                let Some(command) = command else {
                    self.lsp_status = "C#: no server".into();
                    self.log("csharp-ls not found. Install it with: dotnet tool install -g csharp-ls (or set \"csharp.languageServer\"), or use the Roslyn server (\"csharp.server\": \"roslyn\").");
                    return;
                };
                ServerConfig::new(command, Vec::new(), root, "csharp")
            }
        };
        match LspClient::start(config) {
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
                                        // Every view of the file (it can be open in both groups).
                                        for tab in wb.tabs.iter().filter(|t| t.path.as_deref().is_some_and(|p| bf_core::paths::same_path(p, &path))) {
                                            tab.editor.update(cx, |e, cx| e.set_lsp_diagnostics(diagnostics.clone(), cx));
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

    // ───────────── the Roslyn language server ─────────────

    /// How to start the installed Roslyn server for `root` (its solution, else its projects). Offers to
    /// install it when it isn't, and says what's missing.
    fn roslyn_server_config(&mut self, root: &Path, cx: &mut Context<Self>) -> Option<ServerConfig> {
        let cache = self.paths.cache_dir();
        let Some((version, dll)) = bf_core::roslyn::installed(&cache).into_iter().next() else {
            self.lsp_status = "C#: Roslyn not installed".into();
            self.log("The Roslyn language server isn't installed. Run \"C#: Install or Update Roslyn Language Server\" (it downloads it from Microsoft's NuGet feed), or set \"csharp.server\" back to \"csharp-ls\".");
            self.show_panel(PanelView::Output, cx);
            return None;
        };
        if !bf_core::roslyn::runtime_available() {
            self.lsp_status = "C#: needs .NET 10".into();
            self.log("The Roslyn language server needs the .NET 10 runtime or newer (dotnet --list-runtimes).");
            self.show_panel(PanelView::Output, cx);
            return None;
        }
        let logs = cache.join("roslyn-logs");
        let _ = std::fs::create_dir_all(&logs);
        let target = bf_core::dotnet::find_target(root, None, false);
        let solution = target.as_deref().filter(|t| matches!(t.extension().and_then(|e| e.to_str()), Some("sln" | "slnx")));
        let projects = if solution.is_some() { Vec::new() } else { bf_core::dotnet::projects(root) };
        self.log(format!("C#: Roslyn language server {version}"));
        Some(bf_lsp::roslyn_config(&dll, root.to_path_buf(), &logs, solution, &projects))
    }

    /// "C#: Install or Update Roslyn Language Server": the newest version from Microsoft's feed, checked
    /// against Microsoft's signature; then switches to it (setting csharp.server) and restarts.
    pub(super) fn install_roslyn(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cache = self.paths.cache_dir();
        self.lsp_status = "C#: getting Roslyn…".into();
        self.log("Looking for the newest Roslyn language server on Microsoft's NuGet feed…");
        self.show_panel(PanelView::Output, cx);
        let task = cx.background_spawn(async move {
            let (base, version) = bf_core::roslyn::latest()?;
            let installed = bf_core::roslyn::installed(&cache);
            if installed.first().is_some_and(|(v, _)| *v == version) {
                return Ok((version, false));
            }
            bf_core::roslyn::install(&cache, &base, &version)?;
            // Older versions aren't needed anymore.
            for (old, dll) in installed {
                if old != version
                    && let Some(dir) = dll.parent()
                {
                    let _ = std::fs::remove_dir_all(dir);
                }
            }
            anyhow::Ok((version, true))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |wb, cx| {
                match result {
                    Ok((version, fresh)) => {
                        wb.log(if fresh { format!("Installed the Roslyn language server {version} (signed by Microsoft).") } else { format!("The Roslyn language server {version} is the newest.") });
                        if wb.settings.get_string("csharp.server").as_deref() != Some("roslyn") {
                            wb.settings.set("csharp.server", json!("roslyn"));
                            wb.log("C# now uses the Roslyn language server (\"csharp.server\": \"roslyn\"; \"csharp-ls\" switches back).");
                        }
                        wb.start_lsp(cx);
                    }
                    Err(e) => {
                        wb.lsp_status = "C#: Roslyn install failed".into();
                        wb.log(format!("Could not install the Roslyn language server: {e:#}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ───────────── code actions ─────────────

    /// Applies a quick fix or refactoring: resolves its edit if the server computes it lazily, applies the
    /// edit, runs its command, then does the follow-up in `editor` (the one it was picked in).
    pub(super) fn apply_code_action(&mut self, action: bf_lsp::CodeAction, then: AfterApply, editor: gpui::WeakEntity<EditorView>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(lsp) = self.lsp.clone() else { return };
        // Lazily computed actions carry `data` instead of an edit.
        let resolve = action.edit.is_none() && (action.needs_resolve() || action.raw.get("data").is_some_and(|d| !d.is_null()));
        if !resolve {
            self.finish_code_action(action, then, editor, window, cx);
            return;
        }
        let rx = lsp.resolve_code_action(&action);
        cx.spawn_in(window, async move |this, cx| {
            let result = rx.recv_async().await;
            let _ = this.update_in(cx, |wb, window, cx| match result {
                Ok(Ok(resolved)) => wb.finish_code_action(resolved, then, editor, window, cx),
                Ok(Err(e)) => wb.log(format!("Could not apply \"{}\": {e:#}", action.title)),
                Err(_) => {}
            });
        })
        .detach();
    }

    fn finish_code_action(&mut self, action: bf_lsp::CodeAction, then: AfterApply, editor: gpui::WeakEntity<EditorView>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(edit) = action.edit.clone() {
            let title = action.display_title();
            self.confirm_workspace_edit(&title, &edit, window, cx, move |wb, window, cx| wb.apply_code_action_now(action, then, editor, window, cx));
        } else {
            self.apply_code_action_now(action, then, editor, window, cx);
        }
    }

    /// Before an edit that changes several files: asks, listing the files and how many places in each
    /// ("refactor.previewMultiFileChanges"). One-file edits apply right away; undo takes them back.
    pub(super) fn confirm_workspace_edit(
        &mut self,
        title: &str,
        edit: &bf_lsp::WorkspaceEdit,
        window: &mut Window,
        cx: &mut Context<Self>,
        apply: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let files: Vec<(PathBuf, usize)> = edit.changes.iter().filter(|(_, e)| !e.is_empty()).map(|(p, e)| (p.clone(), e.len())).collect();
        let creates = edit.creates.iter().filter(|p| !files.iter().any(|(f, _)| f == *p)).count();
        if files.len() + creates <= 1 || !self.settings.get("refactor.previewMultiFileChanges", true) {
            return apply(self, window, cx);
        }
        let root = self.workspace.root();
        let places: usize = files.iter().map(|(_, n)| n).sum();
        let mut lines: Vec<String> = files
            .iter()
            .take(12)
            .map(|(p, n)| {
                let shown = root.as_deref().and_then(|r| p.strip_prefix(r).ok()).unwrap_or(p).display().to_string();
                let new = if p.exists() { "" } else { " (new file)" };
                format!("{shown}: {n} change{}{new}", if *n == 1 { "" } else { "s" })
            })
            .collect();
        if files.len() > 12 {
            lines.push(format!("… and {} more files", files.len() - 12));
        }
        let question = format!("{title}: {places} change{} in {} files", if places == 1 { "" } else { "s" }, files.len() + creates);
        let detail = format!("{}\n\nFiles that aren't open are opened with the changes, unsaved, so you can review them.", lines.join("\n"));
        let rx = window.prompt(PromptLevel::Info, &question, Some(&detail), &["Apply", "Cancel"], cx);
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(0) = rx.await {
                let _ = this.update_in(cx, |wb, window, cx| apply(wb, window, cx));
            }
        })
        .detach();
    }

    fn apply_code_action_now(&mut self, action: bf_lsp::CodeAction, then: AfterApply, editor: gpui::WeakEntity<EditorView>, window: &mut Window, cx: &mut Context<Self>) {
        let editor = editor.upgrade();
        // The file as it was, to tell the names a refactoring made up from the ones already there.
        let before = editor.as_ref().and_then(|e| {
            let e = e.read(cx);
            Some((e.path.clone()?, e.state.buffer.text()))
        });
        if let Some(edit) = &action.edit {
            self.apply_workspace_edit(edit, Some(window), cx);
        }
        if let (Some(command), Some(lsp)) = (&action.command, &self.lsp) {
            let _ = lsp.execute_command(command);
        }
        if action.edit.is_none() && action.command.is_none() {
            self.log(format!("\"{}\" made no changes.", action.title));
            return;
        }
        let Some(editor) = editor else { return };
        if then == AfterApply::SortUsings {
            editor.update(cx, |e, cx| e.sort_usings(cx));
        }
        // Like Visual Studio: after Extract Method (Introduce Local, Extract Interface, …) rename what it made up.
        if crate::editor::invents_name(&action.title)
            && self.settings.get("refactor.renameGenerated", true)
            && let (Some(edit), Some((path, old))) = (&action.edit, before)
            && let Some((_, edits)) = edit.changes.iter().find(|(p, _)| bf_core::paths::same_path(p, &path))
            && let Some((name, line, col)) = crate::editor::generated_name(&old, edits)
        {
            editor.update(cx, |e, cx| e.rename_generated(&name, line, col, cx));
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
            // Refactorings like Extract Interface write to a new file without asking for it to be created.
            if !path.exists() && !edits.is_empty() && !self.tabs.iter().any(|t| t.path.as_deref().is_some_and(|p| bf_core::paths::same_path(p, &path))) {
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                if let Err(e) = std::fs::write(&path, "") {
                    self.log(format!("Could not create {}: {e}", path.display()));
                    continue;
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workbench::settings_page::view_tests::workbench;

    /// With "csharp.server": "roslyn", opening a C# folder starts the installed Roslyn server on its
    /// project, and an open file gets its diagnostics. Needs the server installed in a cache folder: set
    /// BF_ROSLYN_CACHE (as `cargo test -p bf-core --test roslyn_install -- --ignored` leaves it); skips
    /// otherwise.
    #[gpui::test]
    fn the_roslyn_server_serves_a_folder(cx: &mut gpui::TestAppContext) {
        let Ok(cache) = std::env::var("BF_ROSLYN_CACHE") else {
            println!("SKIPPED: set BF_ROSLYN_CACHE to a cache with the Roslyn server installed");
            return;
        };
        let (guard, wb, cx) = workbench(cx, "roslyn");
        let dir = guard.dir.join("app");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("App.csproj"), "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net10.0</TargetFramework><ImplicitUsings>enable</ImplicitUsings></PropertyGroup></Project>").unwrap();
        let program = dir.join("Program.cs");
        std::fs::write(&program, "int x = \"s\";\nConsole.WriteLine(x);\n").unwrap();
        assert!(std::process::Command::new("dotnet").args(["restore", "-v", "q"]).current_dir(&dir).status().unwrap().success());

        wb.update_in(cx, |wb, window, cx| {
            wb.paths.cache_override = Some(PathBuf::from(&cache));
            wb.settings.set("csharp.server", json!("roslyn"));
            wb.open_folder(dir.clone(), window, cx);
            wb.open_file(program.clone(), None, window, cx);
        });
        // The server loads the project and the editor gets the error on line 1.
        let mut found = false;
        for _ in 0..240 {
            cx.run_until_parked();
            found = wb.update(cx, |wb, cx| {
                wb.active_editor().is_some_and(|e| e.read(cx).diagnostics.iter().any(|d| d.code.as_deref() == Some("CS0029")))
            });
            if found {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        let status = wb.update(cx, |wb, _| wb.lsp_status.clone());
        assert!(found, "no CS0029 in the editor (status: {status})");
        assert_eq!(status, "C#: ready");
        wb.update(cx, |wb, _| {
            if let Some(lsp) = wb.lsp.take() {
                lsp.shutdown();
            }
        });
        drop(guard);
    }
}
