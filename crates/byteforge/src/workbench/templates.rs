//! New Project / New Item from templates (`dotnet new`, plus ByteForge's own C# class, interface, record,
//! enum and struct), and Manage User Secrets.

use bf_core::templates::{self, CSharpItem, Template, TemplateKind};

use super::*;

/// SDK item templates that take `--namespace` (the rest generate their own or have none).
const NAMESPACE_TEMPLATES: &[&str] = &["apicontroller", "mvccontroller", "page", "viewimports", "proto"];

/// A palette row that does nothing when picked.
fn note(text: impl Into<String>) -> PaletteItem {
    PaletteItem::new(text, PaletteAction::GoToLine(usize::MAX))
}

/// Files directly in `dir`, to find what a template created.
fn files_in(dir: &Path) -> std::collections::HashSet<PathBuf> {
    std::fs::read_dir(dir).map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_file()).collect()).unwrap_or_default()
}

impl Workbench {
    /// New Project (`project`) or New Item (in `dir`, else next to the active file, else the workspace root).
    pub(super) fn open_template_picker(&mut self, project: bool, dir: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.root() else {
            self.log(if project { "Open a folder first: new projects are created in it." } else { "Open a folder first." });
            self.show_panel(PanelView::Output, cx);
            return;
        };
        let dir = dir
            .or_else(|| self.active.and_then(|ix| self.tabs.get(ix)).and_then(|t| t.path.as_ref()?.parent().map(Path::to_path_buf)).filter(|d| d.starts_with(&root)))
            .unwrap_or_else(|| root.clone());
        let dir_arg = dir.to_string_lossy().into_owned();
        let title = if project {
            format!("New project in {}", root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
        } else {
            format!("New item in {}", dir.strip_prefix(&root).ok().filter(|r| !r.as_os_str().is_empty()).map_or_else(|| ".".to_owned(), |r| r.display().to_string()))
        };
        let cached = if project { self.project_templates.clone() } else { self.item_templates.clone() };
        let items = match &cached {
            Some(list) => Self::template_items(list, project, &dir_arg),
            None => {
                let mut items = if project { Vec::new() } else { Self::template_items(&[], false, &dir_arg) };
                items.push(note("Loading templates from dotnet new…"));
                items
            }
        };
        self.menu_open = None;
        let palette = cx.new(|cx| Palette::new(Mode::Pick, items, &title, "", window, cx));
        let sub = cx.subscribe_in(&palette, window, Self::on_palette_event);
        palette.update(cx, |p, cx| p.focus_input(window, cx));
        self.palette = Some((palette.clone(), sub));
        cx.notify();
        if cached.is_some() {
            return;
        }
        let kind = if project { TemplateKind::Project } else { TemplateKind::Item };
        let weak = palette.downgrade();
        self.template_task = Some(cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { templates::list(kind) }).await;
            let _ = this.update(cx, |wb, cx| {
                let items = match result {
                    Ok(list) => {
                        let items = Self::template_items(&list, project, &dir_arg);
                        if project {
                            wb.project_templates = Some(list);
                        } else {
                            wb.item_templates = Some(list);
                        }
                        items
                    }
                    Err(e) => {
                        let mut items = if project { Vec::new() } else { Self::template_items(&[], false, &dir_arg) };
                        items.push(note(format!("Could not list templates: {e:#}")));
                        items
                    }
                };
                // Only if that palette is still the open one.
                if let Some(palette) = weak.upgrade()
                    && wb.palette.as_ref().is_some_and(|(p, _)| *p == palette)
                {
                    palette.update(cx, |p, cx| p.set_items(Mode::Pick, items, cx));
                }
            });
        }));
    }

    fn template_items(list: &[Template], project: bool, dir: &str) -> Vec<PaletteItem> {
        let mut items = Vec::new();
        if !project {
            for item in CSharpItem::ALL {
                let action = UseTemplate { template: format!("csharp:{}", item.keyword()), title: item.label().into(), project: false, dir: dir.into() };
                let mut row = PaletteItem::new(item.label(), PaletteAction::Command(Box::new(action)));
                row.detail = Some(format!("C# {} in its own file", item.keyword()));
                row.hint = Some("C#".into());
                items.push(row);
            }
        }
        for t in list {
            let action = UseTemplate { template: t.short_name.clone(), title: t.name.clone(), project, dir: dir.into() };
            let mut row = PaletteItem::new(t.name.clone(), PaletteAction::Command(Box::new(action)));
            row.detail = Some(if t.tags.is_empty() { t.short_name.clone() } else { format!("{} · {}", t.short_name, t.tags) });
            row.search = Some(format!("{} {} {}", t.name, t.short_name, t.tags));
            items.push(row);
        }
        items
    }

    /// A template was picked: ask for the name (config files like `.gitignore` need none).
    pub(super) fn use_template(&mut self, template: UseTemplate, window: &mut Window, cx: &mut Context<Self>) {
        let unnamed = !template.project
            && self.item_templates.as_ref().and_then(|l| l.iter().find(|t| t.short_name == template.template)).is_some_and(|t| t.tags.starts_with("Config") || t.tags.starts_with("MSBuild"));
        if unnamed {
            if let Err(e) = self.create_from_template(template, String::new(), window, cx) {
                self.log(format!("Operation failed: {e}"));
                self.show_panel(PanelView::Output, cx);
            }
            return;
        }
        self.open_prompt(PromptKind::Template(template), "", window, cx);
    }

    pub(super) fn create_from_template(&mut self, template: UseTemplate, name: String, window: &mut Window, cx: &mut Context<Self>) -> anyhow::Result<()> {
        let root = self.workspace.root().ok_or_else(|| anyhow::anyhow!("no folder is open"))?;
        if let Some(keyword) = template.template.strip_prefix("csharp:") {
            let item = CSharpItem::from_keyword(keyword).ok_or_else(|| anyhow::anyhow!("unknown item {keyword}"))?;
            let dir = PathBuf::from(&template.dir);
            let type_name = templates::type_name(&name);
            let path = dir.join(format!("{type_name}.cs"));
            if path.exists() {
                anyhow::bail!("{} already exists", path.display());
            }
            std::fs::create_dir_all(&dir)?;
            std::fs::write(&path, item.render(templates::namespace_for(&dir).as_deref(), &type_name))?;
            self.explorer.update(cx, |e, cx| e.refresh(cx));
            self.open_file(path, None, window, cx);
            return Ok(());
        }
        if self.dotnet_busy(cx) {
            return Ok(());
        }
        self.show_panel(PanelView::Output, cx);
        if template.project {
            let folder = root.join(name.trim());
            if folder.exists() {
                anyhow::bail!("{} already exists", folder.display());
            }
            let args = vec!["new".into(), template.template.clone(), "-n".into(), name.trim().to_owned(), "-o".into(), folder.to_string_lossy().into_owned()];
            let sln_root = root.clone();
            let then: DotnetDone = Box::new(move |wb, ok, window, cx| {
                let root = sln_root;
                if !ok {
                    return;
                }
                let project = bf_core::dotnet::projects(&folder).into_iter().next();
                let open = move |wb: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>| {
                    wb.explorer.update(cx, |e, cx| e.refresh(cx));
                    wb.solution.update(cx, |s, cx| s.load(cx));
                    let program = folder.join("Program.cs");
                    if let Some(file) = Some(program).filter(|p| p.is_file()).or_else(|| bf_core::dotnet::projects(&folder).into_iter().next()) {
                        wb.open_file(file, None, window, cx);
                    }
                };
                match (templates::solution_in(&root), project) {
                    (Some(sln), Some(project)) => {
                        let args = vec!["sln".into(), sln.to_string_lossy().into_owned(), "add".into(), project.to_string_lossy().into_owned()];
                        wb.start_dotnet(root.clone(), args, Some(Box::new(move |wb, _, window, cx| open(wb, window, cx))), window, cx);
                    }
                    _ => open(wb, window, cx),
                }
            });
            self.start_dotnet(root.clone(), args, Some(then), window, cx);
        } else {
            let dir = PathBuf::from(&template.dir);
            let before = files_in(&dir);
            let mut args = vec!["new".into(), template.template.clone(), "-o".into(), dir.to_string_lossy().into_owned()];
            if !name.trim().is_empty() {
                args.extend(["-n".into(), name.trim().to_owned()]);
            }
            if NAMESPACE_TEMPLATES.contains(&template.template.as_str())
                && let Some(ns) = templates::namespace_for(&dir)
            {
                args.extend(["--namespace".into(), ns]);
            }
            let then: DotnetDone = Box::new(move |wb, ok, window, cx| {
                wb.explorer.update(cx, |e, cx| e.refresh(cx));
                if !ok {
                    return;
                }
                let mut created: Vec<PathBuf> = files_in(&dir).difference(&before).cloned().collect();
                created.sort();
                for file in created {
                    wb.open_file(file, None, window, cx);
                }
            });
            self.start_dotnet(root.clone(), args, Some(then), window, cx);
        }
        Ok(())
    }

    // ───────────── user secrets ─────────────

    /// "Manage User Secrets": opens secrets.json of the active file's project (else the startup project),
    /// running `dotnet user-secrets init` first when the project has no UserSecretsId yet.
    pub(super) fn open_user_secrets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.root() else {
            self.log("Open a folder with a .NET project first.");
            self.show_panel(PanelView::Output, cx);
            return;
        };
        let active = self.active.and_then(|ix| self.tabs.get(ix)).and_then(|t| t.path.clone());
        let Some(project) = active.and_then(|p| super::nuget::owning_project(&p, &root)).or_else(|| self.startup_project()) else {
            self.log("No .csproj found. Open a file of the project whose secrets you want to edit.");
            self.show_panel(PanelView::Output, cx);
            return;
        };
        if let Some(id) = templates::user_secrets_id(&project) {
            return self.open_secrets_page(project, &id, window, cx);
        }
        if self.dotnet_busy(cx) {
            return;
        }
        let args = vec!["user-secrets".into(), "init".into(), "--project".into(), project.to_string_lossy().into_owned()];
        let then: DotnetDone = Box::new(move |wb, ok, window, cx| match templates::user_secrets_id(&project) {
            Some(id) if ok => wb.open_secrets_page(project.clone(), &id, window, cx),
            _ => wb.show_panel(PanelView::Output, cx),
        });
        self.start_dotnet(root, args, Some(then), window, cx);
    }

    /// The secrets as a form in a tab (reused when it is open).
    fn open_secrets_page(&mut self, project: PathBuf, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(file) = templates::user_secrets_file(id) else {
            self.log("Could not find the user secrets folder (HOME / APPDATA is not set).");
            return;
        };
        if let Some(ix) = self.tabs.iter().position(|t| matches!(&t.page, Some(Page::Secrets { file: f, .. }) if *f == file)) {
            return self.activate(ix, window, cx);
        }
        let name = project.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let editor = cx.new(|cx| {
            let mut e = EditorView::new("", None, cx);
            e.read_only = true;
            e
        });
        self.add_tab(editor, None, format!("User Secrets: {name}"), false, window, cx);
        if let Some(tab) = self.tabs.last_mut() {
            tab.page = Some(Page::Secrets { project, file });
        }
        cx.notify();
    }

    /// Sets (or, with `None`, removes) one secret in `file`.
    pub(super) fn update_secret(&mut self, file: &Path, key: &str, value: Option<&str>, cx: &mut Context<Self>) -> anyhow::Result<()> {
        let mut secrets = templates::read_secrets(file)?;
        templates::set_secret(&mut secrets, key, value);
        templates::write_secrets(file, &secrets)?;
        self.reload_changed_tabs(cx);
        cx.notify();
        Ok(())
    }

    pub(super) fn render_secrets_page(&mut self, project: &Path, file: &Path, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::global(cx);
        let (fg, muted, rule) = (t.c("editor.foreground"), t.c("foreground.muted"), t.c("editorWidget.border"));
        let hover = t.c("toolbar.hoverBackground");
        let shown = self.secrets_shown.contains(file);
        let name = project.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let secrets = templates::read_secrets(file);
        let button = |id: &'static str, label: &'static str| {
            div().id(id).debug_selector(move || id.into()).px(px(10.)).py(px(4.)).rounded(px(5.)).border_1().border_color(rule).cursor_pointer().hover(move |s| s.bg(hover)).text_size(px(12.)).child(label)
        };
        let (add_file, show_file, json_file) = (file.to_path_buf(), file.to_path_buf(), file.to_path_buf());
        let header = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().text_size(px(20.)).font_weight(gpui::FontWeight::SEMIBOLD).child(format!("User secrets of {name}")))
            .child(div().text_size(px(12.)).text_color(muted).child(format!("Kept outside the project in {}; read in Development as configuration (Section:Key).", file.display())))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .mt_2()
                    .child(button("secret-add", "Add Secret").on_click(cx.listener(move |wb, _, w, cx| wb.open_prompt(PromptKind::SecretNew { file: add_file.clone() }, "", w, cx))))
                    .child(button("secret-show", if shown { "Hide Values" } else { "Show Values" }).on_click(cx.listener(move |wb, _, _, cx| {
                        if !wb.secrets_shown.remove(&show_file) {
                            wb.secrets_shown.insert(show_file.clone());
                        }
                        cx.notify();
                    })))
                    .child(button("secret-json", "Open secrets.json").on_click(cx.listener(move |wb, _, w, cx| {
                        if !json_file.exists() {
                            let _ = templates::write_secrets(&json_file, &[]);
                        }
                        wb.open_file(json_file.clone(), None, w, cx);
                    }))),
            );
        let body: gpui::AnyElement = match secrets {
            Err(e) => div().text_color(t.c("errorForeground")).child(format!("{e:#}. Fix it in secrets.json.")).into_any_element(),
            Ok(list) if list.is_empty() => div().text_color(muted).child("No secrets yet. Add one, e.g. ConnectionStrings:Default.").into_any_element(),
            Ok(list) => div()
                .flex()
                .flex_col()
                .border_1()
                .border_color(rule)
                .rounded(px(6.))
                .children(list.into_iter().enumerate().map(|(i, (key, value))| {
                    let (edit_file, edit_key, edit_value) = (file.to_path_buf(), key.clone(), value.clone());
                    let (del_file, del_key) = (file.to_path_buf(), key.clone());
                    let masked = if shown { value.clone() } else if value.is_empty() { String::new() } else { "\u{2022}".repeat(value.chars().count().clamp(6, 12)) };
                    div()
                        .id(SharedString::from(format!("secret-{i}")))
                        .debug_selector(|| format!("secret-row-{key}"))
                        .group("secret-row")
                        .flex()
                        .items_center()
                        .gap_3()
                        .px_3()
                        .py(px(6.))
                        .when(i > 0, |d| d.border_t_1().border_color(rule))
                        .hover(move |s| s.bg(hover))
                        .child(div().w(px(280.)).flex_none().overflow_hidden().whitespace_nowrap().font_family(t.editor_font.clone()).child(key.clone()))
                        .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().font_family(t.editor_font.clone()).text_color(if shown { fg } else { muted }).child(masked))
                        .child(
                            div()
                                .id(SharedString::from(format!("secret-edit-{i}")))
                                .debug_selector(|| format!("secret-edit-{key}"))
                                .p(px(3.))
                                .rounded(px(4.))
                                .cursor_pointer()
                                .hover(move |s| s.bg(hover))
                                .child(icon("pencil", px(14.), muted))
                                .tooltip(tooltip("Change the value"))
                                .on_click(cx.listener(move |wb, _, w, cx| wb.open_prompt(PromptKind::SecretValue { file: edit_file.clone(), key: edit_key.clone() }, &edit_value, w, cx))),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("secret-delete-{i}")))
                                .debug_selector(|| format!("secret-delete-{key}"))
                                .p(px(3.))
                                .rounded(px(4.))
                                .cursor_pointer()
                                .hover(move |s| s.bg(hover))
                                .child(icon("trash", px(14.), muted))
                                .tooltip(tooltip("Delete this secret"))
                                .on_click(cx.listener(move |_, _, w, cx| {
                                    let rx = w.prompt(PromptLevel::Warning, &format!("Delete the secret {del_key}?"), None, &["Delete", "Cancel"], cx);
                                    let (file, key) = (del_file.clone(), del_key.clone());
                                    cx.spawn_in(w, async move |this, cx| {
                                        if let Ok(0) = rx.await {
                                            let _ = this.update(cx, |wb, cx| {
                                                if let Err(e) = wb.update_secret(&file, &key, None, cx) {
                                                    wb.log(format!("Could not delete the secret: {e:#}"));
                                                }
                                            });
                                        }
                                    })
                                    .detach();
                                })),
                        )
                }))
                .into_any_element(),
        };
        div()
            .id("secrets-page")
            .size_full()
            .overflow_y_scroll()
            .bg(t.c("editor.background"))
            .text_color(fg)
            .text_size(px(13.))
            .child(div().max_w(px(900.)).p_8().flex().flex_col().gap_4().child(header).child(body))
    }

    // ───────────── launch profile in the status bar ─────────────

    /// Re-reads the startup project's launch profile for the status bar (cheap enough for file changes,
    /// too slow for every frame).
    pub(super) fn refresh_status_profile(&mut self) {
        self.status_profile = self.startup_project().and_then(|project| {
            let profiles = bf_core::debug::launch_profiles(&project);
            let first = profiles.first()?.name.clone();
            Some(self.chosen_launch_profile(&project).unwrap_or(first))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workbench::settings_page::view_tests::workbench;

    /// A secrets.json under a test-only id (the real secrets folder is in the user's home); its folder is
    /// removed on drop.
    struct SecretsFile(PathBuf);

    impl Drop for SecretsFile {
        fn drop(&mut self) {
            if let Some(dir) = self.0.parent() {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    }

    fn app(dir: &Path, secrets_id: Option<&str>) -> PathBuf {
        let project = dir.join("App");
        std::fs::create_dir_all(&project).unwrap();
        let id = secrets_id.map(|id| format!("\n    <UserSecretsId>{id}</UserSecretsId>")).unwrap_or_default();
        let csproj = format!("<Project Sdk=\"Microsoft.NET.Sdk\">\n  <PropertyGroup>\n    <OutputType>Exe</OutputType>\n    <TargetFramework>net11.0</TargetFramework>{id}\n  </PropertyGroup>\n</Project>\n");
        std::fs::write(project.join("App.csproj"), csproj).unwrap();
        std::fs::write(project.join("Program.cs"), "System.Console.WriteLine(\"hi\");\n").unwrap();
        project
    }

    fn selector(name: String) -> &'static str {
        Box::leak(name.into_boxed_str())
    }

    fn click(cx: &mut gpui::VisualTestContext, name: &str) {
        cx.run_until_parked();
        let bounds = cx.debug_bounds(selector(name.to_owned())).unwrap_or_else(|| panic!("{name} is drawn"));
        cx.simulate_click(bounds.center(), gpui::Modifiers::none());
        cx.run_until_parked();
    }

    /// Types into the open prompt and presses Enter.
    fn answer(cx: &mut gpui::VisualTestContext, text: &str) {
        cx.simulate_input(text);
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
    }

    fn shown(cx: &mut gpui::VisualTestContext, keys: &[&str]) -> Vec<bool> {
        cx.run_until_parked();
        keys.iter().map(|k| cx.debug_bounds(selector(format!("secret-row-{k}"))).is_some()).collect()
    }

    /// Manage User Secrets lists the secrets (nested ones as Section:Key) and adds, changes and deletes
    /// them through the page's buttons, writing secrets.json.
    #[gpui::test]
    fn the_secrets_page_lists_adds_edits_and_deletes(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "secrets");
        let id = format!("byteforge-test-{}", std::process::id());
        let file = SecretsFile(templates::user_secrets_file(&id).unwrap());
        std::fs::create_dir_all(file.0.parent().unwrap()).unwrap();
        std::fs::write(&file.0, "{\n  // local only\n  \"Db\": { \"Password\": \"hunter2\" },\n  \"Token\": \"t\",\n}\n").unwrap();
        let ws = guard.dir.join("ws");
        let project = app(&ws, Some(&id));

        wb.update_in(cx, |wb, window, cx| wb.open_folder(ws.clone(), window, cx));
        cx.run_until_parked();
        wb.update_in(cx, |wb, window, cx| wb.open_file(project.join("Program.cs"), None, window, cx));
        cx.run_until_parked();
        cx.dispatch_action(ManageUserSecrets);
        cx.run_until_parked();
        wb.read_with(cx, |wb, cx| {
            let tabs: Vec<_> = wb.tabs.iter().map(|t| (t.title.clone(), t.page.is_some())).collect();
            let page = wb.active.and_then(|ix| wb.tabs[ix].page.clone());
            assert!(matches!(page, Some(Page::Secrets { .. })), "active {:?} of {tabs:?}; output:\n{}", wb.active, wb.output.read(cx).state.buffer.text());
        });
        assert_eq!(shown(cx, &["Db:Password", "Token"]), [true, true]);
        let read = || templates::read_secrets(&file.0).unwrap();

        // Add Secret: "Name = value" (the value may contain '=').
        click(cx, "secret-add");
        answer(cx, "ConnectionStrings:Default = Server=x;Database=Shop");
        assert!(read().contains(&("ConnectionStrings:Default".into(), "Server=x;Database=Shop".into())), "{:?}", read());
        assert_eq!(shown(cx, &["ConnectionStrings:Default"]), [true]);

        // The pencil changes a value (the prompt starts with the old one), also to an empty one.
        click(cx, "secret-edit-Token");
        cx.simulate_keystrokes("backspace");
        answer(cx, "");
        assert!(read().contains(&("Token".into(), String::new())), "{:?}", read());
        assert!(wb.read_with(cx, |wb, _| wb.palette.is_none()));
        click(cx, "secret-edit-Token");
        answer(cx, "new-token");
        assert!(read().contains(&("Token".into(), "new-token".into())), "{:?}", read());

        // The trash can asks, then deletes.
        click(cx, "secret-delete-Db:Password");
        cx.simulate_prompt_answer("Delete");
        cx.run_until_parked();
        assert_eq!(read().iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), ["Token", "ConnectionStrings:Default"]);
        // (Not asserted on the drawing: GPUI's test frames never forget a debug selector once drawn.)

        // Show Values toggles the masking.
        click(cx, "secret-show");
        assert!(wb.read_with(cx, |wb, _| wb.secrets_shown.contains(&file.0)));
        drop(guard);
    }

    /// A project without a UserSecretsId gets one from `dotnet user-secrets init`, then the (empty) page
    /// opens and the first secret creates secrets.json. Skips without the .NET SDK.
    #[gpui::test]
    fn a_project_without_secrets_is_initialized(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "secrets-init");
        let ws = guard.dir.join("ws");
        let project = app(&ws, None);
        let csproj = project.join("App.csproj");
        wb.update_in(cx, |wb, window, cx| wb.open_folder(ws.clone(), window, cx));
        cx.run_until_parked();
        wb.update_in(cx, |wb, window, cx| wb.open_file(project.join("Program.cs"), None, window, cx));
        cx.run_until_parked();
        cx.dispatch_action(ManageUserSecrets);
        let mut id = None;
        for _ in 0..300 {
            cx.run_until_parked();
            id = templates::user_secrets_id(&csproj);
            if id.is_some() && !wb.read_with(cx, |wb, _| wb.dotnet.is_some()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let Some(id) = id else {
            eprintln!("skipped: dotnet user-secrets init did not run (no .NET SDK?)");
            return;
        };
        let file = SecretsFile(templates::user_secrets_file(&id).unwrap());
        cx.run_until_parked();
        assert!(wb.read_with(cx, |wb, _| matches!(wb.active.and_then(|ix| wb.tabs[ix].page.clone()), Some(Page::Secrets { .. }))), "the page opens after init");
        click(cx, "secret-add");
        answer(cx, "Api:Key = 42");
        assert_eq!(templates::read_secrets(&file.0).unwrap(), [("Api:Key".to_owned(), "42".to_owned())]);
        assert_eq!(shown(cx, &["Api:Key"]), [true]);
        drop(guard);
    }
}
