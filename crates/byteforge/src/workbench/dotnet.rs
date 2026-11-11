//! Running dotnet commands and collecting build diagnostics.

use super::*;

impl Workbench {
    pub(super) fn run_dotnet(&mut self, verb: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.root() else {
            self.log("Open a folder containing a .NET project first.");
            self.show_panel(PanelView::Output, cx);
            return;
        };
        if self.dotnet_busy(cx) {
            return;
        }
        let runnable = matches!(verb, "run" | "watch");
        let configured = self.settings.get_string("dotnet.project");
        let Some(target) = find_target(&root, configured.as_deref(), runnable) else {
            self.log(format!("No .sln, .slnx or .csproj found in {}.", root.display()));
            self.show_panel(PanelView::Output, cx);
            return;
        };
        self.save_all(window, cx);
        self.show_panel(PanelView::Output, cx);
        self.start_dotnet_verb(root, verb, &target, window, cx);
    }

    /// Starts `dotnet build|clean|restore <target>`, `dotnet run --project <target>` or, for "watch",
    /// `dotnet watch run --project <target>` (hot reload). Run and watch use the project's launch profile.
    pub(super) fn start_dotnet_verb(&mut self, root: PathBuf, verb: &str, target: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let target_arg = target.to_string_lossy().into_owned();
        let mut env = Vec::new();
        let mut on_line: Option<LineHook> = None;
        let args = match verb {
            "run" | "watch" => {
                let mut args = if verb == "watch" { vec!["watch".to_owned(), "run".into()] } else { vec!["run".to_owned()] };
                args.extend(["--project".into(), target_arg]);
                if let Some(profile) = self.chosen_launch_profile(target) {
                    args.extend(["--launch-profile".into(), profile]);
                }
                self.watch_target = (verb == "watch").then(|| target.to_path_buf());
                if verb == "watch" {
                    // No terminal to answer "restart?" prompts: restart by itself after edits hot reload can't apply.
                    env.push(("DOTNET_WATCH_RESTART_ON_RUDE_EDIT".to_owned(), "1".to_owned()));
                    env.push(("DOTNET_WATCH_SUPPRESS_EMOJIS".to_owned(), "1".to_owned()));
                    // Each rebuild or hot reload attempt replaces the problems of the last one, and errors show
                    // as they arrive (the process keeps running).
                    let root = root.clone();
                    on_line = Some(Box::new(move |wb: &mut Workbench, line: &str, _: &mut Window, cx: &mut Context<Workbench>| {
                        let attempt = is_watch_attempt(line);
                        if attempt {
                            wb.build_problems.clear();
                        }
                        if attempt || bf_core::debug::parse_build_line(strip_watch_prefix(line), &root).is_some() {
                            wb.apply_build_diagnostics(cx);
                            cx.notify();
                        }
                    }));
                }
                args
            }
            _ => vec![verb.to_owned(), target_arg],
        };
        self.start_dotnet_with(root, args, env, on_line, None, window, cx);
    }

    /// "Restart Watch": stops `dotnet watch` and starts it again (a full rebuild and a fresh process, what
    /// Ctrl+R does in a terminal; the watch here has no terminal to read keys from).
    pub(super) fn restart_watch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let running = self.dotnet.as_ref().is_some_and(DotnetProcess::is_running);
        let Some(target) = self.watch_target.clone().filter(|_| running) else {
            if running {
                self.log("Restart Watch: the running process isn't dotnet watch. Stop it first (Shift+F5).");
                return;
            }
            return self.run_dotnet("watch", window, cx);
        };
        let Some(root) = self.workspace.root() else { return };
        if let Some(process) = &self.dotnet {
            process.kill();
        }
        self.log("Restarting dotnet watch…");
        cx.spawn_in(window, async move |this, cx| {
            // Wait for the old process to be gone (it gets a few seconds to stop).
            for _ in 0..80 {
                cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
                let gone = this.update(cx, |wb, _| wb.dotnet.as_ref().is_none_or(|d| !d.is_running())).unwrap_or(true);
                if gone {
                    break;
                }
            }
            let _ = this.update_in(cx, |wb, window, cx| {
                wb.dotnet = None;
                wb.save_all(window, cx);
                wb.start_dotnet_verb(root, "watch", &target, window, cx);
            });
        })
        .detach();
    }

    // ───────────── launch profiles ─────────────

    fn launch_profile_key(project: &Path) -> String {
        format!("launchProfile:{}", project.display())
    }

    /// The launch profile picked for `project` (else the `dotnet.launchProfile` setting), if the project
    /// still has it. `None` means the default: the first profile in launchSettings.json.
    pub(super) fn chosen_launch_profile(&self, project: &Path) -> Option<String> {
        let wanted = self.state.get(&Self::launch_profile_key(project)).or_else(|| self.settings.get_string("dotnet.launchProfile")).filter(|n| !n.is_empty())?;
        bf_core::debug::launch_profiles(project).iter().any(|p| p.name == wanted).then_some(wanted)
    }

    /// The project F5 and Ctrl+F5 start.
    pub(super) fn startup_project(&self) -> Option<PathBuf> {
        let root = self.workspace.root()?;
        let configured = self.settings.get_string("dotnet.project");
        find_target(&root, configured.as_deref(), true).filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("csproj")))
    }

    /// "Select Launch Profile…": the startup project's profiles from Properties/launchSettings.json.
    pub(super) fn open_launch_profile_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.startup_project() else {
            self.log("No .csproj to run in this folder. Set \"dotnet.project\" to the project to start.");
            self.show_panel(PanelView::Output, cx);
            return;
        };
        let profiles = bf_core::debug::launch_profiles(&project);
        let name = project.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if profiles.is_empty() {
            self.log(format!("{name} has no launch profiles (Properties/launchSettings.json with \"commandName\": \"Project\")."));
            self.show_panel(PanelView::Output, cx);
            return;
        }
        let current = self.chosen_launch_profile(&project).unwrap_or_else(|| profiles[0].name.clone());
        let project_arg = project.to_string_lossy().into_owned();
        let items = profiles
            .into_iter()
            .map(|p| {
                let mark = if p.name == current { "● " } else { "   " };
                let mut item = PaletteItem::new(format!("{mark}{}", p.name), PaletteAction::Command(Box::new(UseLaunchProfile { project: project_arg.clone(), name: p.name.clone() })));
                item.detail = Some(profile_summary(&p));
                item.search = Some(p.name.clone());
                item
            })
            .collect();
        self.menu_open = None;
        let palette = cx.new(|cx| Palette::new(Mode::Pick, items, &format!("Launch profile for {name} (F5, {}, {})", crate::actions::key_label("ctrl-f5"), crate::actions::key_label("alt-f5")), "", window, cx));
        let sub = cx.subscribe_in(&palette, window, Self::on_palette_event);
        palette.update(cx, |p, cx| p.focus_input(window, cx));
        self.palette = Some((palette, sub));
        cx.notify();
    }

    pub(super) fn use_launch_profile(&mut self, project: &Path, name: &str, cx: &mut Context<Self>) {
        self.state.set(&Self::launch_profile_key(project), Some(name));
        let project_name = project.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.log(format!("{project_name} now runs with launch profile \"{name}\"."));
        self.refresh_status_profile();
        cx.notify();
    }

    /// Logs and returns true when a dotnet process (or a debug session) is already running.
    pub(super) fn dotnet_busy(&mut self, cx: &mut Context<Self>) -> bool {
        let busy = self.dotnet.as_ref().is_some_and(DotnetProcess::is_running) || self.debug.is_some();
        if busy {
            self.log("A dotnet process or debug session is already running. Stop it first (Shift+F5).");
            self.show_panel(PanelView::Output, cx);
        }
        busy
    }

    /// Runs `dotnet <args>` in `root` with output streamed to the Output panel. Compiler errors and
    /// warnings go to Problems. `then` runs when the process exits, with whether it succeeded.
    pub(super) fn start_dotnet(&mut self, root: PathBuf, args: Vec<String>, then: Option<DotnetDone>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.start_dotnet_with(root, args, Vec::new(), None, then, window, cx)
    }

    /// [`start_dotnet`](Self::start_dotnet) with extra environment variables and a hook called with each
    /// output line.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn start_dotnet_with(
        &mut self,
        root: PathBuf,
        args: Vec<String>,
        env: Vec<(String, String)>,
        mut on_line: Option<LineHook>,
        then: Option<DotnetDone>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let verb = args.first().cloned().unwrap_or_default();
        let compiles = matches!(verb.as_str(), "build" | "run" | "test" | "watch");
        if compiles {
            self.build_problems.clear();
        }
        self.log(format!("> dotnet {}", args.join(" ")));
        match DotnetProcess::start_with_env(&root, args, &env) {
            Ok((process, rx)) => {
                self.dotnet = Some(process);
                let tx = self.output_tx.clone();
                self.dotnet_task = Some(cx.spawn_in(window, async move |this, cx| {
                    while let Ok(event) = rx.recv_async().await {
                        match event {
                            RunEvent::Line(line) => {
                                if compiles && let Some(d) = bf_core::debug::parse_build_line(strip_watch_prefix(&line), &root) {
                                    let _ = this.update(cx, |wb, _| wb.add_build_problem(d));
                                }
                                if let Some(hook) = on_line.as_mut() {
                                    let _ = this.update_in(cx, |wb, window, cx| hook(wb, &line, window, cx));
                                }
                                let _ = tx.send(line);
                            }
                            RunEvent::Exited { code, seconds } => {
                                let _ = tx.send(log_line(format!("dotnet {verb} exited with code {} after {seconds:.1}s", code.map_or("?".into(), |c| c.to_string()))));
                                let _ = this.update_in(cx, |wb, window, cx| {
                                    wb.dotnet = None;
                                    if compiles {
                                        wb.apply_build_diagnostics(cx);
                                    }
                                    if let Some(then) = then {
                                        then(wb, code == Some(0), window, cx);
                                    }
                                    cx.notify();
                                });
                                break;
                            }
                        }
                    }
                }));
                cx.notify();
                true
            }
            Err(e) => {
                self.log(format!("Could not start dotnet (is the .NET SDK installed?): {e}"));
                cx.notify();
                false
            }
        }
    }

    pub(super) fn add_build_problem(&mut self, d: bf_core::debug::BuildDiagnostic) {
        let line = d.line.saturating_sub(1) as usize;
        let col = d.column.saturating_sub(1) as usize;
        let diagnostic = crate::editor::Diagnostic {
            start: Point::new(line, col),
            end: Point::new(line, col + 1),
            severity: match d.severity {
                bf_core::debug::BuildSeverity::Error => bf_lsp::Severity::Error,
                bf_core::debug::BuildSeverity::Warning => bf_lsp::Severity::Warning,
            },
            message: d.message,
            code: d.code,
        };
        let path = normalize_path(&d.path);
        // MSBuild repeats every diagnostic in its summary.
        let duplicate = self.build_problems.iter().any(|(p, x)| {
            *p == path && x.start == diagnostic.start && x.message == diagnostic.message && x.code == diagnostic.code
        });
        if !duplicate {
            self.build_problems.push((path, diagnostic));
        }
    }

    /// Whether build diagnostics stand in for a language server in this editor (Razor files, or C#
    /// without csharp-ls).
    pub(super) fn uses_build_diagnostics(&self, editor: &EditorView) -> bool {
        editor.language != Some(bf_syntax::Language::CSharp) || self.lsp.is_none()
    }

    /// Shows the last build's diagnostics in open editors that have no language server.
    pub(super) fn apply_build_diagnostics(&self, cx: &mut Context<Self>) {
        for tab in &self.tabs {
            let Some(path) = &tab.path else { continue };
            if !self.uses_build_diagnostics(tab.editor.read(cx)) {
                continue;
            }
            let diagnostics: Vec<_> = self.build_problems.iter().filter(|(p, _)| p == path).map(|(_, d)| d.clone()).collect();
            tab.editor.update(cx, |e, cx| e.set_diagnostics(diagnostics, cx));
        }
    }

    // ───────────── terminal ─────────────
}

/// A `dotnet watch` line without its "dotnet watch : " prefix (hot reload reports compiler errors with it).
fn strip_watch_prefix(line: &str) -> &str {
    line.strip_prefix("dotnet watch").and_then(|rest| rest.trim_start().strip_prefix(':')).map_or(line, str::trim_start)
}

/// Whether a `dotnet watch` line starts a new build or hot reload attempt.
fn is_watch_attempt(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("dotnet watch") else { return false };
    let rest = rest.to_lowercase();
    rest.contains("building") || rest.contains("file updated") || rest.contains("file changed")
}

/// One line about a launch profile for the picker: its URLs, arguments and environment.
fn profile_summary(p: &bf_core::debug::LaunchProfile) -> String {
    let mut parts = Vec::new();
    if let Some(urls) = &p.application_url {
        parts.push(urls.replace(';', "  "));
    }
    if !p.args.is_empty() {
        parts.push(format!("args: {}", p.args.join(" ")));
    }
    if let Some((_, env)) = p.env.iter().find(|(k, _)| k == "ASPNETCORE_ENVIRONMENT" || k == "DOTNET_ENVIRONMENT") {
        parts.push(env.clone());
    }
    if parts.is_empty() { "no URLs or arguments".into() } else { parts.join("  ·  ") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_watch_lines() {
        assert!(is_watch_attempt("dotnet watch : Building /src/W/W.csproj ..."));
        assert!(is_watch_attempt("dotnet watch : File updated: ./Program.cs"));
        assert!(!is_watch_attempt("dotnet watch : [W (net10.0)] Hot reload succeeded."));
        assert!(!is_watch_attempt("info: building something"));
        let error = "dotnet watch : /src/W/Program.cs(7,9): error CS0029: Cannot implicitly convert type 'string' to 'int'";
        assert_eq!(strip_watch_prefix(error), "/src/W/Program.cs(7,9): error CS0029: Cannot implicitly convert type 'string' to 'int'");
        let d = bf_core::debug::parse_build_line(strip_watch_prefix(error), Path::new("/src/W")).unwrap();
        assert_eq!((d.line, d.column, d.code.as_deref()), (7, 9, Some("CS0029")));
        assert_eq!(strip_watch_prefix("Now listening on: http://localhost:5039"), "Now listening on: http://localhost:5039");
    }
}
