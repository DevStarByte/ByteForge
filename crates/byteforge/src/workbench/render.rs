//! Top-level window rendering, sash dragging and window resize edges.

use super::*;

impl Render for Workbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::global(cx);
        let title = match self.active.and_then(|i| self.tabs.get(i)) {
            Some(tab) => {
                let dirty = if tab.editor.read(cx).is_modified() { "● " } else { "" };
                format!("{dirty}{} — ByteForge", tab.title)
            }
            None => "ByteForge".into(),
        };
        window.set_window_title(&title);
        self.sync_module_view_visibility();
        let tests_visible = self.side_visible && self.side == SideView::Testing;
        self.tests.update(cx, |t, cx| t.set_visible(tests_visible, cx));
        let solution_visible = self.side_visible && self.side == SideView::Solution;
        self.solution.update(cx, |s, cx| s.set_visible(solution_visible, cx));

        // Every region is placed by the theme layout (see layout.rs).
        let lay = &t.layout;
        // Focus is tracked below the title bar: focusing on mouse-down marks the click as handled, which would
        // stop Windows from starting a window drag on the title bar.
        let mut middle = div().track_focus(&self.focus).flex().flex_row().flex_1().min_h_0().min_w_0().overflow_hidden();
        let mut has_editor = false;
        for part in &lay.main {
            match part.as_str() {
                "activityBar" => middle = middle.child(self.render_activity_bar(lay.right_of_editor("activityBar"), false, cx)),
                "sideBar" if self.side_visible => middle = middle.child(self.render_side_bar(lay.right_of_editor("sideBar"), cx)),
                "editor" if !has_editor => {
                    has_editor = true;
                    middle = middle.child(self.render_main(window, cx));
                }
                "panel" if self.panel_visible => {
                    let place = if lay.right_of_editor("panel") { PanelPlace::Right } else { PanelPlace::Left };
                    middle = middle.child(self.render_panel(place, cx));
                }
                _ => {}
            }
        }
        if !has_editor {
            middle = middle.child(self.render_main(window, cx));
        }
        let mut middle = Some(middle);
        let mut rows: Vec<gpui::AnyElement> = Vec::new();
        // A custom title bar carries the window buttons and the drag area, so it can move but not disappear.
        if self.custom_titlebar && !lay.workbench.iter().any(|p| p == "titleBar") {
            rows.push(self.render_title_bar(window, cx).into_any_element());
        }
        for part in &lay.workbench {
            match part.as_str() {
                "titleBar" => rows.push(self.render_title_bar(window, cx).into_any_element()),
                "statusBar" => rows.push(self.render_status_bar(cx).into_any_element()),
                "main" => rows.extend(middle.take().map(|m| m.into_any_element())),
                _ => {}
            }
        }
        rows.extend(middle.take().map(|m| m.into_any_element()));

        let entity = cx.entity();
        let root = div()
            .id("workbench")
            .key_context("Workbench")
            .size_full()
            .flex()
            .flex_col()
            .relative()
            .bg(t.bg("window.background"))
            .text_color(t.c("window.foreground"))
            .font_family(t.ui_font.clone())
            .text_size(t.m("ui.size", 13.0))
            .font_weight(t.weight("ui.weight", gpui::FontWeight::NORMAL))
            .on_mouse_move(cx.listener(|wb, e: &MouseMoveEvent, _, cx| wb.on_sash_drag(e, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|wb, _, _, cx| {
                if wb.sash.take().is_some() {
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|wb, _: &NewFile, w, cx| wb.new_file(w, cx)))
            .on_action(cx.listener(|wb, _: &OpenFile, w, cx| wb.pick_files(w, cx)))
            .on_action(cx.listener(|wb, _: &OpenFolder, w, cx| wb.pick_folder(w, cx)))
            .on_action(cx.listener(|wb, _: &Save, w, cx| {
                if let Some(i) = wb.active {
                    wb.save_formatted(i, w, cx)
                }
            }))
            .on_action(cx.listener(|wb, _: &FormatDocument, w, cx| {
                if let Some(i) = wb.active
                    && !wb.format_tab(i, None, false, w, cx)
                {
                    let name = wb.tabs[i].title.clone();
                    wb.log(format!("No formatter handles {name}. CSharpier formats C# and MSBuild/XML files; csharp-ls formats C#."));
                    wb.show_panel(PanelView::Output, cx);
                }
            }))
            .on_action(cx.listener(|wb, _: &FormatDocumentWith, w, cx| wb.open_formatter_picker(w, cx)))
            .on_action(cx.listener(|wb, action: &UseFormatter, w, cx| {
                wb.settings.set("editor.defaultFormatter", json!(action.id));
                if let Some(i) = wb.active {
                    wb.format_tab(i, Some(action.id.clone()), false, w, cx);
                }
            }))
            .on_action(cx.listener(|wb, _: &SaveAs, w, cx| {
                if let Some(i) = wb.active {
                    wb.save_as(i, w, cx)
                }
            }))
            .on_action(cx.listener(|wb, _: &SaveAll, w, cx| wb.save_all(w, cx)))
            .on_action(cx.listener(|wb, _: &CloseEditor, w, cx| {
                if let Some(i) = wb.active {
                    wb.close_tab(i, w, cx)
                }
            }))
            .on_action(cx.listener(|wb, _: &CloseAllEditors, w, cx| {
                for i in (0..wb.tabs.len()).rev() {
                    wb.close_tab(i, w, cx);
                }
            }))
            .on_action(cx.listener(|wb, _: &Quit, w, cx| {
                if wb.should_close(w, cx) {
                    cx.quit();
                }
            }))
            .on_action(cx.listener(|wb, _: &CommandPalette, w, cx| wb.open_palette(Mode::Commands, w, cx)))
            .on_action(cx.listener(|wb, _: &QuickOpen, w, cx| wb.open_palette(Mode::Files, w, cx)))
            .on_action(cx.listener(|wb, _: &GoToLine, w, cx| wb.open_palette(Mode::GoToLine, w, cx)))
            .on_action(cx.listener(|wb, _: &GoToSymbolInEditor, w, cx| wb.open_palette(Mode::Symbols, w, cx)))
            .on_action(cx.listener(|wb, _: &GoToSymbolInWorkspace, w, cx| wb.open_palette(Mode::WorkspaceSymbols, w, cx)))
            .on_action(cx.listener(|wb, _: &SelectTheme, w, cx| wb.open_palette(Mode::Themes, w, cx)))
            .on_action(cx.listener(|wb, _: &ToggleSideBar, _, cx| {
                wb.side_visible = !wb.side_visible;
                cx.notify();
            }))
            .on_action(cx.listener(|wb, _: &ResetActivityBarOrder, _, cx| wb.reset_activity_order(cx)))
            .on_action(cx.listener(|wb, _: &TogglePanel, _, cx| {
                wb.panel_visible = !wb.panel_visible;
                cx.notify();
            }))
            .on_action(cx.listener(|wb, _: &ShowExplorer, w, cx| wb.toggle_side(SideView::Explorer, w, cx)))
            .on_action(cx.listener(|wb, _: &ShowSearch, w, cx| wb.toggle_side(SideView::Search, w, cx)))
            .on_action(cx.listener(|wb, _: &ShowModules, w, cx| wb.toggle_side(SideView::Modules, w, cx)))
            .on_action(cx.listener(|wb, _: &ShowSourceControl, w, cx| wb.toggle_side(SideView::SourceControl, w, cx)))
            .on_action(cx.listener(|wb, _: &GitRefresh, _, cx| wb.scm.update(cx, |s, cx| s.refresh(cx))))
            .on_action(cx.listener(|wb, _: &GitCommit, w, cx| {
                wb.show_side(SideView::SourceControl, cx);
                wb.scm.update(cx, |s, cx| s.commit(w, cx))
            }))
            .on_action(cx.listener(|wb, _: &GitStageAll, _, cx| wb.scm.update(cx, |s, cx| s.stage_all(cx))))
            .on_action(cx.listener(|wb, _: &GitUnstageAll, _, cx| wb.scm.update(cx, |s, cx| s.unstage_all(cx))))
            .on_action(cx.listener(|wb, _: &GitPull, _, cx| wb.scm.update(cx, |s, cx| s.pull(cx))))
            .on_action(cx.listener(|wb, _: &GitPush, _, cx| wb.scm.update(cx, |s, cx| s.push(cx))))
            .on_action(cx.listener(|wb, _: &GitFetch, _, cx| wb.scm.update(cx, |s, cx| s.fetch(cx))))
            .on_action(cx.listener(|wb, _: &GitSync, _, cx| wb.scm.update(cx, |s, cx| s.sync(cx))))
            .on_action(cx.listener(|wb, _: &GitCheckout, w, cx| wb.open_branch_picker(w, cx)))
            .on_action(cx.listener(|wb, _: &GitCreateBranch, w, cx| wb.open_prompt(PromptKind::CreateBranch, "", w, cx)))
            .on_action(cx.listener(|wb, _: &GitInit, _, cx| {
                wb.show_side(SideView::SourceControl, cx);
                wb.scm.update(cx, |s, cx| s.init(cx))
            }))
            .on_action(cx.listener(|wb, _: &GitFileHistory, w, cx| wb.open_file_history(w, cx)))
            .on_action(cx.listener(|wb, a: &GitShowFileAtCommit, w, cx| wb.open_commit_diff(a.commit.clone(), a.file.clone(), w, cx)))
            .on_action(cx.listener(|wb, _: &ToggleMergeEditor, w, cx| wb.toggle_merge_editor(w, cx)))
            .on_action(cx.listener(|wb, _: &GitToggleBlame, _, cx| wb.toggle_blame(cx)))
            .on_action(cx.listener(|wb, _: &GitToggleBlameGutter, _, cx| {
                // Per editor: the column for the file you're looking at.
                if let Some(editor) = wb.active_editor() {
                    editor.update(cx, |e, cx| {
                        let on = !e.blame_gutter();
                        e.set_blame_gutter(on, cx);
                    });
                }
            }))
            .on_action(cx.listener(|wb, _: &GitOpenChanges, w, cx| {
                let path = wb.active.and_then(|i| wb.tabs.get(i)).and_then(|t| t.path.clone());
                let change = path.and_then(|p| wb.scm.read(cx).status().change_for(&p).cloned());
                match change {
                    Some(c) => {
                        let staged = !c.is_unstaged();
                        wb.open_diff(c, staged, w, cx)
                    }
                    None => wb.log("The current file has no changes."),
                }
            }))
            .on_action(cx.listener(|wb, _: &ShowProblems, _, cx| wb.show_panel(PanelView::Problems, cx)))
            .on_action(cx.listener(|wb, _: &ShowOutput, _, cx| wb.show_panel(PanelView::Output, cx)))
            .on_action(cx.listener(|wb, _: &NextEditor, w, cx| wb.cycle_tab(1, w, cx)))
            .on_action(cx.listener(|wb, _: &SplitEditor, w, cx| wb.split_editor(w, cx)))
            .on_action(cx.listener(|wb, _: &MarkdownOpenPreview, w, cx| wb.open_markdown_preview(false, w, cx)))
            .on_action(cx.listener(|wb, _: &MarkdownOpenPreviewToSide, w, cx| wb.open_markdown_preview(true, w, cx)))
            .on_action(cx.listener(|wb, _: &FocusFirstGroup, w, cx| wb.focus_group(0, w, cx)))
            .on_action(cx.listener(|wb, _: &FocusSecondGroup, w, cx| wb.focus_group(1, w, cx)))
            .on_action(cx.listener(|wb, _: &PreviousEditor, w, cx| wb.cycle_tab(-1, w, cx)))
            .on_action(cx.listener(|wb, _: &CustomizeTheme, w, cx| {
                let name = format!("{} Custom", Theme::global(cx).data.name);
                wb.open_prompt(PromptKind::CustomizeTheme, &name, w, cx)
            }))
            .on_action(cx.listener(|wb, _: &EditTheme, w, cx| {
                if let Some(path) = Theme::global(cx).data.file_path.clone() {
                    wb.open_file(path, None, w, cx);
                }
            }))
            .on_action(cx.listener(|wb, _: &ReloadTheme, w, cx| wb.reload_theme(w, cx)))
            .on_action(cx.listener(|wb, _: &OpenSettings, w, cx| wb.open_settings_page(w, cx)))
            .on_action(cx.listener(|wb, _: &OpenSettingsJson, w, cx| {
                let path = wb.settings.path().to_path_buf();
                wb.open_file(path, None, w, cx)
            }))
            .on_action(cx.listener(|wb, _: &About, _, cx| {
                wb.log(format!("ByteForge {} — Rust + GPUI, {} {}", update::VERSION, std::env::consts::OS, std::env::consts::ARCH));
                wb.log(format!("Config: {}", wb.paths.config_dir.display()));
                wb.show_panel(PanelView::Output, cx);
            }))
            .on_action(cx.listener(|wb, _: &CheckForUpdates, w, cx| wb.check_for_updates(true, w, cx)))
            .on_action(cx.listener(|wb, _: &Build, w, cx| wb.run_dotnet("build", w, cx)))
            .on_action(cx.listener(|wb, _: &Run, w, cx| wb.run_dotnet("run", w, cx)))
            .on_action(cx.listener(|wb, _: &RunWithWatch, w, cx| wb.run_dotnet("watch", w, cx)))
            .on_action(cx.listener(|wb, _: &RestartWatch, w, cx| wb.restart_watch(w, cx)))
            .on_action(cx.listener(|wb, _: &SelectLaunchProfile, w, cx| wb.open_launch_profile_picker(w, cx)))
            .on_action(cx.listener(|wb, a: &UseLaunchProfile, _, cx| wb.use_launch_profile(Path::new(&a.project), &a.name, cx)))
            .on_action(cx.listener(|wb, _: &NewProject, w, cx| wb.open_template_picker(true, None, w, cx)))
            .on_action(cx.listener(|wb, _: &NewItem, w, cx| wb.open_template_picker(false, None, w, cx)))
            .on_action(cx.listener(|wb, a: &UseTemplate, w, cx| wb.use_template(a.clone(), w, cx)))
            .on_action(cx.listener(|wb, _: &ManageUserSecrets, w, cx| wb.open_user_secrets(w, cx)))
            .on_action(cx.listener(|wb, _: &OpenInOtherGroup, w, cx| wb.open_in_other_group(w, cx)))
            .on_action(cx.listener(|wb, _: &ToggleWordWrap, _, cx| {
                // Alt+Z: word wrap on or off for every editor (setting editor.wordWrap).
                let on = !wb.settings.get_string("editor.wordWrap").is_some_and(|w| w == "on");
                wb.settings.set("editor.wordWrap", json!(if on { "on" } else { "off" }));
                for tab in &wb.tabs {
                    tab.editor.update(cx, |e, cx| {
                        if !e.read_only {
                            e.word_wrap = on;
                            cx.notify();
                        }
                    });
                }
            }))
            .on_action(cx.listener(|wb, a: &AddPackageVersion, w, cx| wb.add_package(PathBuf::from(&a.project), a.id.clone(), Some(a.version.clone()), w, cx)))
            .on_action(cx.listener(|wb, _: &EfAddMigration, w, cx| wb.start_ef("add", w, cx)))
            .on_action(cx.listener(|wb, _: &EfRemoveMigration, w, cx| wb.start_ef("remove", w, cx)))
            .on_action(cx.listener(|wb, _: &EfUpdateDatabase, w, cx| wb.start_ef("update", w, cx)))
            .on_action(cx.listener(|wb, _: &EfUpdateDatabaseTo, w, cx| wb.start_ef("update-to", w, cx)))
            .on_action(cx.listener(|wb, _: &EfGenerateScript, w, cx| wb.start_ef("script", w, cx)))
            .on_action(cx.listener(|wb, _: &EfDropDatabase, w, cx| wb.start_ef("drop", w, cx)))
            .on_action(cx.listener(|wb, _: &EfScaffold, w, cx| wb.start_scaffold(w, cx)))
            .on_action(cx.listener(|wb, a: &EfCommand, w, cx| wb.run_ef(a.clone(), w, cx)))
            .on_action(cx.listener(|wb, _: &Test, w, cx| wb.run_all_tests(w, cx)))
            .on_action(cx.listener(|wb, _: &ShowTesting, w, cx| wb.toggle_side(SideView::Testing, w, cx)))
            .on_action(cx.listener(|wb, _: &ShowSolution, w, cx| wb.toggle_side(SideView::Solution, w, cx)))
            .on_action(cx.listener(|wb, _: &AddPackage, w, cx| wb.add_package_command(w, cx)))
            .on_action(cx.listener(|wb, a: &AddPackageTo, w, cx| wb.open_package_search(PathBuf::from(&a.project), w, cx)))
            .on_action(cx.listener(|wb, _: &CheckPackageUpdates, _, cx| {
                wb.show_side(SideView::Solution, cx);
                wb.solution.update(cx, |s, cx| s.check_updates(cx));
            }))
            .on_action(cx.listener(|wb, _: &RunAllTests, w, cx| wb.run_all_tests(w, cx)))
            .on_action(cx.listener(|wb, _: &RerunFailedTests, w, cx| wb.rerun_failed_tests(w, cx)))
            .on_action(cx.listener(|wb, _: &RunTestAtCursor, w, cx| wb.run_test_at_cursor(false, w, cx)))
            .on_action(cx.listener(|wb, _: &DebugTestAtCursor, w, cx| wb.run_test_at_cursor(true, w, cx)))
            .on_action(cx.listener(|wb, a: &ModulePickChoice, _, _| wb.module_pick_choice(a)))
            .on_action(cx.listener(|wb, _: &RefreshTests, _, cx| wb.tests.update(cx, |t, cx| t.discover(cx))))
            .on_action(cx.listener(|wb, _: &Clean, w, cx| wb.run_dotnet("clean", w, cx)))
            .on_action(cx.listener(|wb, _: &Restore, w, cx| wb.run_dotnet("restore", w, cx)))
            .on_action(cx.listener(|wb, _: &Stop, _, cx| {
                if wb.debug.is_some() {
                    wb.debug_stop(cx);
                } else if let Some(d) = &wb.dotnet {
                    d.kill();
                    wb.log("Process stopped.");
                }
                cx.notify();
            }))
            .on_action(cx.listener(|wb, _: &StartDebugging, w, cx| wb.debug_start(w, cx)))
            .on_action(cx.listener(|wb, _: &RestartDebugging, w, cx| wb.debug_restart(w, cx)))
            .on_action(cx.listener(|wb, _: &Pause, _, cx| wb.debug_pause(cx)))
            .on_action(cx.listener(|wb, _: &StepOver, _, cx| wb.debug_step(debug::Step::Over, cx)))
            .on_action(cx.listener(|wb, _: &StepInto, _, cx| wb.debug_step(debug::Step::Into, cx)))
            .on_action(cx.listener(|wb, _: &StepOut, _, cx| wb.debug_step(debug::Step::Out, cx)))
            .on_action(cx.listener(|wb, _: &ToggleBreakpoint, _, cx| {
                if let Some(editor) = wb.active_editor() {
                    editor.update(cx, |e, cx| {
                        if e.glyph_margin {
                            let line = e.state.caret().line;
                            e.toggle_breakpoint(line, cx);
                        }
                    });
                }
            }))
            .on_action(cx.listener(|wb, _: &RemoveAllBreakpoints, _, cx| wb.remove_all_breakpoints(cx)))
            .on_action(cx.listener(|wb, _: &AttachToProcess, w, cx| wb.open_attach_picker(w, cx)))
            .on_action(cx.listener(|wb, action: &AttachToPid, w, cx| wb.debug_attach(action.pid, w, cx)))
            .on_action(cx.listener(|wb, _: &AttachToContainer, w, cx| wb.open_container_picker(w, cx)))
            .on_action(cx.listener(|wb, _: &StartDebuggingInContainer, w, cx| wb.debug_start_in_container(w, cx)))
            .on_action(cx.listener(|wb, _: &AddConditionalBreakpoint, w, cx| wb.edit_breakpoint_option(debug::BreakpointOption::Condition, w, cx)))
            .on_action(cx.listener(|wb, _: &AddLogpoint, w, cx| wb.edit_breakpoint_option(debug::BreakpointOption::LogMessage, w, cx)))
            .on_action(cx.listener(|wb, _: &EditBreakpointHitCount, w, cx| wb.edit_breakpoint_option(debug::BreakpointOption::HitCount, w, cx)))
            .on_action(cx.listener(|wb, a: &PickContainerProcess, w, cx| wb.open_container_process_picker(a.container.clone(), w, cx)))
            .on_action(cx.listener(|wb, a: &AttachToContainerPid, w, cx| wb.debug_attach_container(a.container.clone(), a.pid, w, cx)))
            .on_action(cx.listener(|wb, _: &ShowDebug, w, cx| wb.toggle_side(SideView::Debug, w, cx)))
            .on_action(cx.listener(|wb, _: &ToggleTerminal, w, cx| wb.toggle_terminal(w, cx)))
            .on_action(cx.listener(|wb, _: &NewTerminal, w, cx| wb.new_terminal(w, cx)))
            .on_action(cx.listener(|wb, _: &KillTerminal, w, cx| wb.kill_terminal(w, cx)))
            .on_action(cx.listener(|wb, _: &SelectDefaultShell, w, cx| wb.open_shell_picker(w, cx)))
            .on_action(cx.listener(|wb, action: &SetDefaultShell, w, cx| {
                wb.settings.set("terminal.shell", json!(action.shell));
                wb.new_terminal(w, cx);
            }))
            .on_action(cx.listener(|wb, _: &ShowDebugConsole, w, cx| {
                wb.show_panel(PanelView::DebugConsole, cx);
                let focus = wb.debug_input.focus_handle(cx);
                w.focus(&focus);
            }))
            .on_action(cx.listener(|wb, _: &ToggleLineEnding, _, cx| {
                if let Some(editor) = wb.active_editor() {
                    editor.update(cx, |e, cx| {
                        let next = match e.state.buffer.line_ending() {
                            bf_text::LineEnding::Lf => bf_text::LineEnding::CrLf,
                            bf_text::LineEnding::CrLf => bf_text::LineEnding::Lf,
                        };
                        e.state.buffer.set_line_ending(next);
                        cx.notify();
                    });
                }
                cx.notify();
            }))
            .on_action(cx.listener(|wb, _: &ReloadCSharp, _, cx| wb.start_lsp(cx)))
            .on_action(cx.listener(|wb, _: &InstallRoslyn, w, cx| wb.install_roslyn(w, cx)))
            .on_action(cx.listener(|wb, action: &RunModuleCommand, _, cx| {
                if let Some(module) = wb.plugin_commands.get(&action.id)
                    && let Some(p) = wb.plugins.iter().find(|p| &p.manifest.id == module)
                {
                    p.execute(&action.id);
                }
                cx.notify();
            }))
            .children(rows);

        let _ = entity;
        let root = match &self.palette {
            Some((palette, _)) => root.child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(t.bg("overlay.background"))
                    .flex()
                    .justify_center()
                    .pt(t.edges("palette.margin", 48.0).top)
                    .on_mouse_down(MouseButton::Left, cx.listener(|wb, _, w, cx| {
                        if let Some(name) = wb.palette_restore_theme.take() {
                            wb.apply_theme(&name, w, cx);
                        }
                        wb.close_palette(w, cx)
                    }))
                    .child(div().child(palette.clone())),
            ),
            None => root,
        };
        root.children(self.render_resize_edges(window))
    }
}

impl Workbench {
    pub(super) fn on_sash_drag(&mut self, e: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(sash) = self.sash else { return };
        if e.pressed_button != Some(MouseButton::Left) {
            self.sash = None;
            return;
        }
        match sash {
            Sash::Side { start_x, start_w, right } => {
                let dx = f32::from(e.position.x) - start_x;
                self.side_width = (if right { start_w - dx } else { start_w + dx }).clamp(140.0, 900.0);
            }
            Sash::Panel { start_y, start_h, top } => {
                let dy = f32::from(e.position.y) - start_y;
                self.panel_height = (if top { start_h + dy } else { start_h - dy }).clamp(80.0, 1200.0);
            }
            Sash::PanelSide { start_x, start_w, right } => {
                let dx = f32::from(e.position.x) - start_x;
                self.panel_width = (if right { start_w - dx } else { start_w + dx }).clamp(160.0, 1400.0);
            }
        }
        cx.notify();
    }

    pub(super) fn render_resize_edges(&self, window: &Window) -> Vec<gpui::AnyElement> {
        // Client-side decorations (Linux): resize from the window edges.
        if !matches!(window.window_decorations(), gpui::Decorations::Client { .. }) || window.is_maximized() {
            return Vec::new();
        }
        let edge = |id: &'static str, e: gpui::ResizeEdge, cursor: gpui::CursorStyle| {
            div().id(id).absolute().cursor(cursor).on_mouse_down(MouseButton::Left, move |_, window, cx| {
                cx.stop_propagation();
                window.start_window_resize(e);
            })
        };
        let g = px(5.);
        vec![
            edge("re-t", gpui::ResizeEdge::Top, gpui::CursorStyle::ResizeUpDown).top_0().left(g).right(g).h(g).into_any_element(),
            edge("re-b", gpui::ResizeEdge::Bottom, gpui::CursorStyle::ResizeUpDown).bottom_0().left(g).right(g).h(g).into_any_element(),
            edge("re-l", gpui::ResizeEdge::Left, gpui::CursorStyle::ResizeLeftRight).left_0().top(g).bottom(g).w(g).into_any_element(),
            edge("re-r", gpui::ResizeEdge::Right, gpui::CursorStyle::ResizeLeftRight).right_0().top(g).bottom(g).w(g).into_any_element(),
            edge("re-tl", gpui::ResizeEdge::TopLeft, gpui::CursorStyle::ResizeUpLeftDownRight).top_0().left_0().size(g).into_any_element(),
            edge("re-tr", gpui::ResizeEdge::TopRight, gpui::CursorStyle::ResizeUpRightDownLeft).top_0().right_0().size(g).into_any_element(),
            edge("re-bl", gpui::ResizeEdge::BottomLeft, gpui::CursorStyle::ResizeUpRightDownLeft).bottom_0().left_0().size(g).into_any_element(),
            edge("re-br", gpui::ResizeEdge::BottomRight, gpui::CursorStyle::ResizeUpLeftDownRight).bottom_0().right_0().size(g).into_any_element(),
        ]
    }
}
