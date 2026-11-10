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
        let runnable = verb == "run";
        let configured = self.settings.get_string("dotnet.project");
        let Some(target) = find_target(&root, configured.as_deref(), runnable) else {
            self.log(format!("No .sln, .slnx or .csproj found in {}.", root.display()));
            self.show_panel(PanelView::Output, cx);
            return;
        };
        self.save_all(window, cx);
        let mut args = vec![verb.to_owned()];
        if runnable {
            args.extend(["--project".into(), target.to_string_lossy().into_owned()]);
        } else {
            args.push(target.to_string_lossy().into_owned());
        }
        self.show_panel(PanelView::Output, cx);
        self.start_dotnet(root, args, None, window, cx);
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
        let compiles = matches!(verb.as_str(), "build" | "run" | "test");
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
                                if compiles && let Some(d) = bf_core::debug::parse_build_line(&line, &root) {
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
