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
        // a Roslyn setup still running for the previous folder is dropped (and so cancelled)
        self.lsp_setup = None;
        let has_csharp = self.workspace.files().iter().any(|f| matches!(f.extension().and_then(|e| e.to_str()), Some("cs" | "csproj" | "sln" | "slnx")));
        if !has_csharp {
            self.lsp_status = String::new();
            return;
        }
        if self.settings.get_string("csharp.server").as_deref() == Some("roslyn") {
            // Its checks run `dotnet` and look through the folder, which can take seconds: off the UI thread,
            // so the window stays responsive while ByteForge starts.
            self.lsp_status = "C#: starting…".into();
            let cache = self.paths.cache_dir();
            self.lsp_setup = Some(cx.spawn(async move |this, cx| {
                let setup = cx.background_executor().spawn(async move { roslyn_setup(&cache, &root) }).await;
                let _ = this.update(cx, |wb, cx| match setup {
                    Ok((version, config, razor)) => {
                        wb.log(format!("C#: Roslyn language server {version}{}", if razor { " (with Razor)" } else { "" }));
                        wb.launch_lsp(config, razor, cx);
                    }
                    Err(missing) => {
                        wb.lsp_status = missing.status().into();
                        wb.log(missing.message());
                        wb.show_panel(PanelView::Output, cx);
                        cx.notify();
                    }
                });
            }));
            cx.notify();
            return;
        }
        let command = self.settings.get_string("csharp.languageServer").or_else(bf_lsp::csharp_ls_command);
        let Some(command) = command else {
            self.lsp_status = "C#: no server".into();
            self.log("csharp-ls not found. Install it with: dotnet tool install -g csharp-ls (or set \"csharp.languageServer\"), or use the Roslyn server (\"csharp.server\": \"roslyn\").");
            return;
        };
        let mut config = ServerConfig::new(command, Vec::new(), root, "csharp");
        config.env = bf_lsp::dotnet_server_env();
        self.launch_lsp(config, false, cx);
    }

    /// Whether an editor of `language` talks to the running language server: C# always, Razor when the
    /// server is Roslyn with its Razor extension.
    pub(super) fn lsp_serves(&self, language: Option<bf_syntax::Language>) -> bool {
        self.lsp.is_some() && serves(language, self.lsp_razor)
    }

    /// Asks again for the Code Lens of every open C# file (the server loaded, or reference counts changed).
    pub(super) fn refresh_code_lenses(&mut self, cx: &mut Context<Self>) {
        for tab in &self.tabs {
            tab.editor.update(cx, |e, cx| e.refresh_code_lenses(cx));
        }
    }

    /// Starts the language server and attaches it to the open files it serves (with `razor`, Razor too).
    fn launch_lsp(&mut self, config: ServerConfig, razor: bool, cx: &mut Context<Self>) {
        match LspClient::start(config) {
            Ok((client, events)) => {
                self.lsp_status = "C#: starting…".into();
                self.lsp_razor = razor;
                for tab in &self.tabs {
                    let client = client.clone();
                    tab.editor.update(cx, |e, cx| {
                        if serves(e.language, razor) {
                            // Its build diagnostics give way to the server's.
                            e.set_diagnostics(Vec::new(), cx);
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
                                    LspEvent::Progress(text) => wb.lsp_status = format!("C#: {}", short(&text, 48)),
                                    // by the protocol, not by message text (csharp-ls 0.29 ends with "Loaded app")
                                    LspEvent::Idle => {
                                        wb.lsp_status = "C#: ready".into();
                                        // Counts asked for while the solution loaded were short.
                                        wb.refresh_code_lenses(cx);
                                    }
                                    LspEvent::CodeLensRefresh => wb.refresh_code_lenses(cx),
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

    /// "C#: Install or Update Roslyn Language Server": the newest version from nuget.org, checked against
    /// Microsoft's signature; then switches to it (setting csharp.server) and restarts.
    pub(super) fn install_roslyn(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cache = self.paths.cache_dir();
        self.lsp_status = "C#: getting Roslyn…".into();
        self.log("Looking for the newest Roslyn language server on nuget.org…");
        self.show_panel(PanelView::Output, cx);
        let task = cx.background_spawn(async move {
            bf_core::roslyn::remove_partial_installs(&cache);
            let (base, version) = bf_core::roslyn::latest()?;
            let installed = bf_core::roslyn::installed(&cache);
            if !bf_core::roslyn::is_newer_than_installed(&version, &installed) {
                // Up to date (an installed version can even be newer than what the feed offers).
                return Ok((installed.first().map(|(v, _)| v.clone()).unwrap_or(version), false));
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

/// What keeps the Roslyn server from starting.
enum RoslynMissing {
    NotInstalled,
    Runtime,
}

impl RoslynMissing {
    fn status(&self) -> &'static str {
        match self {
            Self::NotInstalled => "C#: Roslyn not installed",
            Self::Runtime => "C#: needs .NET 10",
        }
    }

    fn message(&self) -> &'static str {
        match self {
            Self::NotInstalled => {
                "The Roslyn language server isn't installed. Run \"C#: Install or Update Roslyn Language Server\" (it downloads it from Microsoft's NuGet feed), or set \"csharp.server\" back to \"csharp-ls\"."
            }
            Self::Runtime => "The Roslyn language server needs the .NET 10 runtime or newer (dotnet --list-runtimes).",
        }
    }
}

/// Whether a server (with or without Razor support) serves files of `language`.
fn serves(language: Option<bf_syntax::Language>, razor: bool) -> bool {
    match language {
        Some(bf_syntax::Language::CSharp) => true,
        Some(bf_syntax::Language::Razor) => razor,
        _ => false,
    }
}

/// How to start the installed Roslyn server for `root` (its solution, else its projects), with its
/// version and whether it serves Razor. Blocking (runs `dotnet` and looks through the folder); run it off
/// the UI thread.
fn roslyn_setup(cache: &Path, root: &Path) -> Result<(String, ServerConfig, bool), RoslynMissing> {
    let (version, dll) = bf_core::roslyn::installed(cache).into_iter().next().ok_or(RoslynMissing::NotInstalled)?;
    if !bf_core::roslyn::runtime_available() {
        return Err(RoslynMissing::Runtime);
    }
    let logs = cache.join("roslyn-logs");
    let _ = std::fs::create_dir_all(&logs);
    let target = bf_core::dotnet::find_target(root, None, false);
    let solution = target.as_deref().filter(|t| matches!(t.extension().and_then(|e| e.to_str()), Some("sln" | "slnx")));
    let projects = if solution.is_some() { Vec::new() } else { bf_core::dotnet::projects(root) };
    let razor = bf_core::roslyn::has_razor(&dll);
    Ok((version, bf_lsp::roslyn_config(&dll, root.to_path_buf(), &logs, solution, &projects), razor))
}

#[cfg(test)]
mod tests {
    use crate::actions::PressKeys;
    use super::*;
    use crate::workbench::settings_page::view_tests::workbench;

    /// A command that logs and shows the Output (like "C#: Install or Update Roslyn Language Server")
    /// brings its new lines into view, also when the Output was scrolled up or clicked into before.
    #[gpui::test]
    fn showing_the_output_follows_new_lines(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "output-follow");
        wb.update(cx, |wb, cx| {
            for i in 0..200 {
                wb.log(format!("earlier line {i}"));
            }
            wb.show_panel(PanelView::Output, cx);
        });
        cx.run_until_parked();
        // The user scrolls up / clicks at the top of the Output: it stops following.
        wb.update(cx, |wb, cx| wb.output.update(cx, |o, _| o.state.set_caret(bf_text::Point::default(), false)));
        // What install_roslyn does first.
        wb.update(cx, |wb, cx| {
            wb.log("Looking for the newest Roslyn language server on Microsoft's NuGet feed…");
            wb.show_panel(PanelView::Output, cx);
        });
        cx.run_until_parked();
        wb.read_with(cx, |wb, cx| {
            assert!(wb.panel_visible && matches!(wb.panel, PanelView::Output));
            let output = wb.output.read(cx);
            let last = output.state.buffer.line_count() - 1;
            assert!(output.state.buffer.text().contains("Looking for the newest Roslyn"));
            assert!(output.state.caret().line + 1 >= last, "the Output follows the new line (caret on {} of {last})", output.state.caret().line);
        });
        drop(guard);
    }

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
        // English whatever the system's language ("locale": "en" in initialize).
        let message = wb.update(cx, |wb, cx| {
            let editor = wb.active_editor().unwrap();
            editor.read(cx).diagnostics.iter().find(|d| d.code.as_deref() == Some("CS0029")).map(|d| d.message.clone()).unwrap_or_default()
        });
        assert!(message.contains("Cannot implicitly convert"), "{message}");
        wb.update(cx, |wb, _| {
            if let Some(lsp) = wb.lsp.take() {
                lsp.shutdown();
            }
        });
        drop(guard);
    }

    /// A Blazor component with the Roslyn server (installed from nuget.org, so with its Razor extension):
    /// the C# error inside `@code` shows in the .razor editor, from the server instead of a build.
    /// Needs BF_ROSLYN_CACHE like the test above; skips otherwise.
    #[gpui::test]
    fn the_roslyn_server_serves_razor_files(cx: &mut gpui::TestAppContext) {
        let Ok(cache) = std::env::var("BF_ROSLYN_CACHE") else {
            println!("SKIPPED: set BF_ROSLYN_CACHE to a cache with the Roslyn server installed");
            return;
        };
        let (guard, wb, cx) = workbench(cx, "roslyn-razor");
        let dir = guard.dir.join("web");
        std::fs::create_dir_all(dir.join("Components")).unwrap();
        std::fs::write(dir.join("Web.csproj"), "<Project Sdk=\"Microsoft.NET.Sdk.Web\"><PropertyGroup><TargetFramework>net10.0</TargetFramework><ImplicitUsings>enable</ImplicitUsings></PropertyGroup></Project>").unwrap();
        std::fs::write(dir.join("Program.cs"), "var app = WebApplication.CreateBuilder(args).Build();\napp.Run();\n").unwrap();
        let component = dir.join("Components").join("Counter.razor");
        std::fs::write(&component, "<h1>@count</h1>\n\n@code {\n    private int count = \"zero\";\n}\n").unwrap();
        assert!(std::process::Command::new("dotnet").args(["restore", "-v", "q"]).current_dir(&dir).status().unwrap().success());

        wb.update_in(cx, |wb, window, cx| {
            wb.paths.cache_override = Some(PathBuf::from(&cache));
            wb.settings.set("csharp.server", json!("roslyn"));
            wb.open_folder(dir.clone(), window, cx);
            wb.open_file(component.clone(), None, window, cx);
        });
        let mut found = false;
        for _ in 0..240 {
            cx.run_until_parked();
            found = wb.update(cx, |wb, cx| {
                wb.active_editor().is_some_and(|e| e.read(cx).diagnostics.iter().any(|d| d.code.as_deref() == Some("CS0029") && d.start.line == 3))
            });
            if found {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        let (status, razor) = wb.update(cx, |wb, _| (wb.lsp_status.clone(), wb.lsp_razor));
        assert!(razor, "the installed server has the Razor extension");
        assert!(found, "no CS0029 on line 4 of the .razor editor (status: {status})");
        wb.update(cx, |wb, _| {
            if let Some(lsp) = wb.lsp.take() {
                lsp.shutdown();
            }
        });
        drop(guard);
    }

    /// The rename prompt opens with the old name selected: typing replaces it (it used to append, so
    /// naming Extract Method's `NewMethod` "Print" made `NewMethodPrint`).
    #[gpui::test]
    fn typing_in_the_rename_prompt_replaces_the_old_name(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "rename-prompt");
        let file = guard.dir.join("notes.txt");
        std::fs::write(&file, "NewMethod\n").unwrap();
        wb.update_in(cx, |wb, window, cx| {
            wb.open_file(file.clone(), None, window, cx);
            let editor = wb.active_editor().unwrap().downgrade();
            wb.open_prompt(PromptKind::RenameSymbol { editor, at: Point::new(0, 0), old: "NewMethod".into() }, "NewMethod", window, cx);
        });
        cx.run_until_parked();
        cx.simulate_input("Print");
        let text = wb.read_with(cx, |wb, cx| wb.palette.as_ref().map(|(p, _)| p.read(cx).input_text(cx)));
        assert_eq!(text.as_deref(), Some("Print"));
        drop(guard);
    }

    // ───────────── the Refactor menu against csharp-ls ─────────────
    // These start the real csharp-ls on a small project and drive the editor by its keys; they skip
    // (pass) with a message when csharp-ls or the .NET SDK isn't installed.

    const CALCULATOR: &str = "using System.Text;
using System;
using System.Linq;

namespace Demo;

public class Calculator
{
    public int Add(int left, int right)
    {
        var sum = left + right;
        Console.WriteLine(sum);
        return sum * 2;
    }
}
";

    /// Polls (letting the server work) until `done` holds or `secs` pass.
    fn wait_for(cx: &mut gpui::VisualTestContext, wb: &Entity<Workbench>, secs: u64, mut done: impl FnMut(&Workbench, &App) -> bool) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
        loop {
            cx.run_until_parked();
            if wb.read_with(cx, |wb, cx| done(wb, cx)) {
                return true;
            }
            if std::time::Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    fn editor_text(wb: &Workbench, cx: &App) -> String {
        wb.active_editor().map(|e| e.read(cx).state.buffer.text()).unwrap_or_default()
    }

    fn menu_lines(wb: &Workbench, cx: &App) -> Option<Vec<String>> {
        wb.active_editor()?.read(cx).action_menu_lines()
    }

    /// The menu once the server answered (not "Looking for …").
    fn menu_after(cx: &mut gpui::VisualTestContext, wb: &Entity<Workbench>) -> Option<Vec<String>> {
        wait_for(cx, wb, 30, |wb, cx| menu_lines(wb, cx).is_none_or(|l| !l.iter().any(|l| l.starts_with("Looking for"))));
        wb.read_with(cx, menu_lines)
    }

    fn output_text(wb: &Workbench, cx: &App) -> String {
        wb.output.read(cx).state.buffer.text()
    }

    /// Opens `files` as a restored C# project with `open` in the editor, and waits for csharp-ls to load it.
    #[allow(clippy::type_complexity)]
    fn csharp_project<'a>(
        cx: &'a mut gpui::TestAppContext,
        name: &str,
        files: &[(&str, &str)],
        open: &str,
    ) -> Option<(crate::workbench::settings_page::view_tests::TestWorkbench, Entity<Workbench>, &'a mut gpui::VisualTestContext, PathBuf)> {
        if bf_lsp::csharp_ls_command().is_none() || std::process::Command::new("dotnet").arg("--version").output().is_err() {
            println!("SKIPPED: csharp-ls or the .NET SDK is not installed");
            return None;
        }
        let (guard, wb, cx) = workbench(cx, name);
        let dir = guard.dir.join("app");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("App.csproj"), "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><OutputType>Library</OutputType><TargetFramework>net10.0</TargetFramework></PropertyGroup></Project>").unwrap();
        for (file, text) in files {
            std::fs::write(dir.join(file), text).unwrap();
        }
        assert!(std::process::Command::new("dotnet").args(["restore", "-v", "q"]).current_dir(&dir).status().unwrap().success());
        let file = dir.join(open);
        wb.update_in(cx, |wb, window, cx| {
            wb.open_folder(dir.clone(), window, cx);
            wb.open_file(file.clone(), None, window, cx);
        });
        let ready = wait_for(cx, &wb, 120, |wb, _| wb.lsp_status == "C#: ready");
        assert!(ready, "csharp-ls didn't load the project (status: {})", wb.read_with(cx, |wb, _| wb.lsp_status.clone()));
        Some((guard, wb, cx, file))
    }

    fn select(cx: &mut gpui::VisualTestContext, wb: &Entity<Workbench>, from: (usize, usize), to: (usize, usize)) {
        wb.update_in(cx, |wb, window, cx| {
            let editor = wb.active_editor().unwrap();
            editor.update(cx, |e, cx| {
                e.state.select(Point::new(from.0, from.1), Point::new(to.0, to.1));
                cx.notify();
            });
            editor.read(cx).focus(window);
        });
    }

    fn stop(cx: &mut gpui::VisualTestContext, wb: &Entity<Workbench>) {
        wb.update(cx, |wb, _| {
            if let Some(lsp) = wb.lsp.take() {
                lsp.shutdown();
            }
        });
    }

    /// Clicks the menu row titled `title`.
    fn click_row(cx: &mut gpui::VisualTestContext, wb: &Entity<Workbench>, title: &str) {
        let lines = wb.read_with(cx, menu_lines).unwrap_or_default();
        let ix = lines.iter().position(|l| l == title).unwrap_or_else(|| panic!("no '{title}' in {lines:?}"));
        // debug_bounds wants a 'static name.
        let row = cx.debug_bounds(Box::leak(format!("action-{ix}").into_boxed_str())).expect("the row is drawn");
        cx.simulate_click(row.center(), gpui::Modifiers::none());
    }

    fn rename_prompt_open(wb: &Workbench) -> bool {
        matches!(wb.prompt, Some(PromptKind::RenameSymbol { .. }))
    }

    /// Waits for the rename prompt, types `name` (over what it suggests) and presses Enter (and Apply, when
    /// the rename changes `several_files`); waits for the rename.
    fn answer_rename(cx: &mut gpui::VisualTestContext, wb: &Entity<Workbench>, name: &str, several_files: bool) {
        assert!(wait_for(cx, wb, 30, |wb, _| rename_prompt_open(wb)), "the rename prompt opens");
        let before = wb.read_with(cx, editor_text);
        cx.simulate_input(name);
        cx.press("enter");
        if several_files {
            apply_dialog(cx, wb);
        }
        let ok = wait_for(cx, wb, 30, |wb, cx| editor_text(wb, cx) != before);
        assert!(ok, "renamed to {name}:\n{}\n{}", wb.read_with(cx, editor_text), wb.read_with(cx, output_text));
    }

    /// Waits for the dialog listing the files a refactoring changes and answers "Apply".
    fn apply_dialog(cx: &mut gpui::VisualTestContext, wb: &Entity<Workbench>) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !cx.has_pending_prompt() && std::time::Instant::now() < deadline {
            wait_for(cx, wb, 0, |_, _| false);
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(cx.has_pending_prompt(), "asks before changing several files");
        cx.simulate_prompt_answer("Apply");
    }

    fn tab_text(wb: &Workbench, path: &Path, cx: &App) -> Option<String> {
        wb.tabs.iter().find(|t| t.path.as_deref() == Some(path)).map(|t| t.editor.read(cx).state.buffer.text())
    }

    /// The Refactor menu lists what fits the selection; picking an entry changes the file and asks for
    /// the name the refactoring made up; a name with the caret right after it can be renamed.
    #[gpui::test]
    fn the_refactor_menu_lists_and_applies_refactorings(cx: &mut gpui::TestAppContext) {
        let Some((guard, wb, cx, _)) = csharp_project(cx, "refactor-menu", &[("Calculator.cs", CALCULATOR)], "Calculator.cs") else { return };
        // An expression: extract and introduce, no Rename, no fixes.
        select(cx, &wb, (10, 18), (10, 30));
        cx.press("ctrl-shift-r");
        let menu = menu_after(cx, &wb).unwrap();
        assert_eq!(menu[..3], ["Extract method", "Extract local function", "Introduce local for 'left + right'"], "{menu:?}");
        assert!(menu.iter().all(|l| l.starts_with("Extract") || l.starts_with("Introduce")), "{menu:?}");
        cx.press("escape");
        // A class name: Rename first, then extracting an interface or a base class.
        select(cx, &wb, (6, 15), (6, 15));
        cx.press("ctrl-shift-r");
        let menu = menu_after(cx, &wb).unwrap();
        assert_eq!(menu[..3], ["Rename 'Calculator'…", "Extract interface...", "Extract base class..."], "{menu:?}");
        cx.press("escape");
        // The caret right after a name (before its parenthesis) is on it.
        select(cx, &wb, (8, 18), (8, 18));
        cx.press("ctrl-shift-r");
        let menu = menu_after(cx, &wb).unwrap();
        assert_eq!(menu.first().map(String::as_str), Some("Rename 'Add'…"), "{menu:?}");
        click_row(cx, &wb, "Rename 'Add'…");
        answer_rename(cx, &wb, "Sum", false);
        assert!(wb.read_with(cx, editor_text).contains("    public int Sum(int left, int right)\n"));

        // Picking Introduce Local introduces it and asks for its name.
        select(cx, &wb, (10, 18), (10, 30));
        cx.press("ctrl-shift-r");
        menu_after(cx, &wb);
        click_row(cx, &wb, "Introduce local for 'left + right'");
        answer_rename(cx, &wb, "total", false);
        let text = wb.read_with(cx, editor_text);
        assert!(text.contains("total = left + right;") && text.contains("sum = total;"), "{text}");
        stop(cx, &wb);
        drop(guard);
    }

    /// Code Lens: "1 reference" shows above Add (resolved by csharp-ls); clicking it lists the references.
    #[gpui::test]
    fn code_lens_counts_and_lists_references(cx: &mut gpui::TestAppContext) {
        let program = "namespace Demo;\n\npublic static class Program\n{\n    public static int Run() => new Calculator().Add(1, 2);\n}\n";
        let Some((guard, wb, cx, _)) = csharp_project(cx, "code-lens", &[("Calculator.cs", CALCULATOR), ("Program.cs", program)], "Calculator.cs") else { return };
        let add_lens = |wb: &Workbench, cx: &App| {
            let editor = wb.active_editor()?.read(cx);
            let ix = editor.lens_hits.iter().position(|h| h.line == 8)?;
            let row = editor.lens_rows.iter().find(|r| r.line == 8)?;
            (row.items.first()?.title == "1 reference").then_some(ix)
        };
        // The lenses are asked for after a pause (the test clock moves only when told to).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while !wb.read_with(cx, |wb, cx| add_lens(wb, cx).is_some()) && std::time::Instant::now() < deadline {
            cx.executor().advance_clock(std::time::Duration::from_millis(500));
            wait_for(cx, &wb, 0, |_, _| false);
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(
            wb.read_with(cx, |wb, cx| add_lens(wb, cx).is_some()),
            "no \"1 reference\" lens above Add: {:?}",
            wb.read_with(cx, |wb, cx| wb.active_editor().map(|e| e.read(cx).lens_rows.clone()))
        );
        let at = wb.read_with(cx, |wb, cx| {
            let editor = wb.active_editor().unwrap().read(cx);
            let hit = &editor.lens_hits[add_lens(wb, cx).unwrap()];
            let l = editor.layout;
            let x = l.text_left - editor.scroll.0 + (hit.x0 + hit.x1) / 2.0;
            let y = l.top + (editor.row_of(8) - 1) as f32 * l.line_height - editor.scroll.1 + l.line_height / 2.0;
            gpui::point(gpui::px(x), gpui::px(y))
        });
        cx.simulate_click(at, gpui::Modifiers::none());
        let listed = wait_for(cx, &wb, 30, |wb, _| wb.references.as_ref().is_some_and(|r| r.items.len() == 2));
        assert!(listed, "declaration and the call in Program.cs listed:\n{}", wb.read_with(cx, output_text));
        stop(cx, &wb);
        drop(guard);
    }

    /// Show Call Hierarchy on Add lists Run (in Program.cs) as its caller; Callees on Run lists Add; Show Type
    /// Hierarchy on Calculator lists the class deriving from it, and clicking it opens its file.
    #[gpui::test]
    fn call_and_type_hierarchy(cx: &mut gpui::TestAppContext) {
        let program = "namespace Demo;\n\npublic static class Program\n{\n    public static int Run() => new Calculator().Add(1, 2);\n}\n\npublic class Scientific : Calculator\n{\n}\n";
        let Some((guard, wb, cx, _)) = csharp_project(cx, "hierarchy", &[("Calculator.cs", CALCULATOR), ("Program.cs", program)], "Calculator.cs") else { return };
        let names = |wb: &Workbench, _: &App| -> Vec<String> {
            let Some(h) = &wb.hierarchy else { return Vec::new() };
            h.visible().iter().map(|&ix| format!("{}{}", "  ".repeat(h.nodes[ix].depth), h.nodes[ix].item.name)).collect()
        };
        select(cx, &wb, (8, 16), (8, 16));
        cx.press("shift-alt-h");
        let found = wait_for(cx, &wb, 60, |wb, cx| names(wb, cx).len() == 2);
        assert!(found, "callers of Add: {:?}\n{}", wb.read_with(cx, names), wb.read_with(cx, output_text));
        let listed = wb.read_with(cx, names);
        assert!(listed[0].starts_with("Add") && listed[1].starts_with("  Run"), "{listed:?}");
        assert!(wb.read_with(cx, |wb, _| wb.panel == PanelView::Hierarchy && wb.hierarchy.as_ref().unwrap().title() == "callers of 'Add'"));

        // Run's own callers: none.
        wb.update(cx, |wb, cx| wb.toggle_hierarchy_node(1, cx));
        assert!(wait_for(cx, &wb, 30, |wb, _| wb.hierarchy.as_ref().unwrap().nodes[1].children.is_some()));
        assert_eq!(wb.read_with(cx, |wb, _| wb.hierarchy.as_ref().unwrap().nodes[1].children.clone()), Some(vec![]));

        // What Add calls: Console.WriteLine has no source here, so nothing from the solution.
        wb.update(cx, |wb, cx| wb.set_hierarchy_direction(false, cx));
        assert!(wait_for(cx, &wb, 30, |wb, _| wb.hierarchy.as_ref().is_some_and(|h| !h.up && h.nodes[0].children.is_some())));

        select(cx, &wb, (6, 15), (6, 15));
        cx.dispatch_action(crate::actions::ShowTypeHierarchy);
        let found = wait_for(cx, &wb, 30, |wb, cx| wb.hierarchy.as_ref().is_some_and(|h| !h.calls) && names(wb, cx).len() == 2);
        assert!(found, "derived types of Calculator: {:?}\n{}", wb.read_with(cx, names), wb.read_with(cx, output_text));
        assert!(wb.read_with(cx, names)[1].starts_with("  Scientific"), "{:?}", wb.read_with(cx, names));
        wb.update_in(cx, |wb, window, cx| wb.open_hierarchy_node(1, window, cx));
        assert!(wait_for(cx, &wb, 10, |wb, cx| wb.active_editor().is_some_and(|e| e.read(cx).path.as_ref().is_some_and(|p| p.ends_with("Program.cs")))));
        stop(cx, &wb);
        drop(guard);
    }

    /// Extract Method by its keys on a statement line (no selection): the method is made and named.
    #[gpui::test]
    fn extract_method_by_key_names_the_new_method(cx: &mut gpui::TestAppContext) {
        let Some((guard, wb, cx, _)) = csharp_project(cx, "refactor-extract", &[("Calculator.cs", CALCULATOR)], "Calculator.cs") else { return };
        select(cx, &wb, (11, 10), (11, 10));
        cx.press("ctrl-r ctrl-m");
        answer_rename(cx, &wb, "Print", false);
        let text = wb.read_with(cx, editor_text);
        assert!(text.contains("        Print(sum);\n") && text.contains("    private static void Print(int sum)\n    {\n        Console.WriteLine(sum);\n    }\n"), "{text}");
        stop(cx, &wb);
        drop(guard);
    }

    /// Inline takes the local back out; a refactoring that doesn't fit the caret says so and what to do.
    #[gpui::test]
    fn inline_and_unavailable_refactorings(cx: &mut gpui::TestAppContext) {
        let Some((guard, wb, cx, _)) = csharp_project(cx, "refactor-inline", &[("Calculator.cs", CALCULATOR)], "Calculator.cs") else { return };
        select(cx, &wb, (12, 15), (12, 15));
        cx.dispatch_action(crate::actions::ExtractInterface);
        assert_eq!(menu_after(cx, &wb), Some(vec!["Extract Interface isn't available here. Put the caret on the name of a class.".to_owned()]));
        cx.press("escape");

        select(cx, &wb, (10, 12), (10, 12));
        cx.dispatch_action(crate::actions::InlineSymbol);
        assert!(wait_for(cx, &wb, 30, |wb, cx| editor_text(wb, cx) != CALCULATOR));
        let text = wb.read_with(cx, editor_text);
        assert!(text.contains("    {\n        Console.WriteLine(left + right);\n        return (left + right) * 2;\n    }"), "{text}");
        assert!(!wb.read_with(cx, |wb, _| rename_prompt_open(wb)), "inlining makes up no name");

        // The `2`: no field (Roslyn makes constants of constants), but a constant, picked and named.
        select(cx, &wb, (11, 32), (11, 33));
        cx.dispatch_action(crate::actions::IntroduceField);
        assert_eq!(menu_after(cx, &wb), Some(vec!["Introduce Field isn't available for this selection.".to_owned()]));
        cx.press("escape");
        cx.dispatch_action(crate::actions::IntroduceConstant);
        menu_after(cx, &wb);
        click_row(cx, &wb, "Introduce constant for '2'");
        answer_rename(cx, &wb, "Factor", false);
        let text = wb.read_with(cx, editor_text);
        assert!(text.contains("const int Factor = 2;") && text.contains("return (left + right) * Factor;"), "{text}");
        stop(cx, &wb);
        drop(guard);
    }

    /// Extract Interface writes the interface to a new file after asking; Extract Base Class (one file)
    /// applies at once. Both ask for the new type's name.
    #[gpui::test]
    fn extract_interface_and_base_class(cx: &mut gpui::TestAppContext) {
        let Some((guard, wb, cx, file)) = csharp_project(cx, "refactor-types", &[("Calculator.cs", CALCULATOR)], "Calculator.cs") else { return };
        select(cx, &wb, (6, 15), (6, 15));
        cx.press("ctrl-r ctrl-i");
        apply_dialog(cx, &wb);
        let interface = file.with_file_name("ICalculator.cs");
        answer_rename(cx, &wb, "IAdder", true);
        let text = wb.read_with(cx, editor_text);
        assert!(text.contains("public class Calculator : IAdder\n"), "{text}");
        let interface_text = wb.read_with(cx, |wb, cx| tab_text(wb, &interface, cx));
        assert!(interface_text.as_deref().is_some_and(|t| t.contains("public interface IAdder") && t.contains("int Add(int left, int right);")), "{interface_text:?}");

        select(cx, &wb, (6, 15), (6, 15));
        cx.dispatch_action(crate::actions::ExtractBaseClass);
        answer_rename(cx, &wb, "AdderBase", false);
        let text = wb.read_with(cx, editor_text);
        assert!(text.contains("public class AdderBase\n{\n    public int Add(") && text.contains("public class Calculator : AdderBase, IAdder\n{\n}"), "{text}");
        stop(cx, &wb);
        drop(guard);
    }

    /// Remove and Sort Usings drops the unused ones (one variant: no menu) and sorts the rest.
    #[gpui::test]
    fn remove_and_sort_usings(cx: &mut gpui::TestAppContext) {
        let text = CALCULATOR.replace("using System.Linq;", "using System.Linq;\nusing System.Collections.Generic;").replace("Console.WriteLine(sum);", "Console.WriteLine(new List<int> { sum }.Count);");
        let Some((guard, wb, cx, _)) = csharp_project(cx, "refactor-usings", &[("Calculator.cs", &text)], "Calculator.cs") else { return };
        // The unused usings come as diagnostics; the command needs them.
        wait_for(cx, &wb, 30, |wb, cx| wb.active_editor().is_some_and(|e| e.read(cx).diagnostics.iter().any(|d| d.code.as_deref() == Some("CS8019"))));
        select(cx, &wb, (0, 0), (0, 0));
        cx.press("ctrl-r ctrl-g");
        let ok = wait_for(cx, &wb, 30, |wb, cx| editor_text(wb, cx).starts_with("using System;\nusing System.Collections.Generic;\n\nnamespace Demo;"));
        assert!(ok, "{}\n{:?}", wb.read_with(cx, editor_text), wb.read_with(cx, menu_lines));
        stop(cx, &wb);
        drop(guard);
    }

    /// Move Type to Matching File offers its variants; moving writes the type to its own file (after
    /// asking, as two files change).
    #[gpui::test]
    fn move_type_to_its_own_file(cx: &mut gpui::TestAppContext) {
        let shapes = "namespace Demo;\n\npublic class Circle\n{\n}\n\npublic class Square\n{\n    public int Side => 2;\n}\n";
        let Some((guard, wb, cx, file)) = csharp_project(cx, "refactor-move", &[("Shapes.cs", shapes)], "Shapes.cs") else { return };
        select(cx, &wb, (6, 15), (6, 15));
        cx.dispatch_action(crate::actions::MoveTypeToFile);
        assert_eq!(menu_after(cx, &wb), Some(vec!["Move type to Square.cs".to_owned(), "Rename file to Square.cs".to_owned(), "Rename type to Shapes".to_owned()]));
        click_row(cx, &wb, "Move type to Square.cs");
        apply_dialog(cx, &wb);
        let square = file.with_file_name("Square.cs");
        let moved = wait_for(cx, &wb, 30, |wb, cx| tab_text(wb, &square, cx).is_some_and(|t| t.contains("public class Square")) && tab_text(wb, &file, cx).is_some_and(|t| !t.contains("Square")));
        assert!(moved, "{:?}\n{:?}", wb.read_with(cx, |wb, cx| tab_text(wb, &file, cx)), wb.read_with(cx, |wb, cx| tab_text(wb, &square, cx)));
        stop(cx, &wb);
        drop(guard);
    }
}
