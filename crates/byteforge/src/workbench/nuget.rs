//! NuGet in the workbench: the package search in the palette, and adding, updating and removing package
//! references with `dotnet add/remove package` (output in the Output panel, the Solution view reloads).

use std::time::Duration;

use super::*;

/// A palette row that does nothing when picked.
fn note(text: impl Into<String>) -> PaletteItem {
    PaletteItem::new(text, PaletteAction::GoToLine(usize::MAX))
}

fn project_name(project: &Path) -> String {
    project.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

impl Workbench {
    /// "NuGet: Add Package…": for the project of the active file, the only project, or one picked from a list.
    pub(super) fn add_package_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.root() else {
            self.log("Open a folder with a .NET project first.");
            self.show_panel(PanelView::Output, cx);
            return;
        };
        let active = self.active.and_then(|ix| self.tabs.get(ix)).and_then(|t| t.path.clone());
        if let Some(project) = active.and_then(|p| owning_project(&p, &root)) {
            return self.open_package_search(project, window, cx);
        }
        let projects = bf_core::dotnet::projects(&root);
        match projects.as_slice() {
            [] => {
                self.log(format!("No .csproj found in {}.", root.display()));
                self.show_panel(PanelView::Output, cx);
            }
            [one] => self.open_package_search(one.clone(), window, cx),
            _ => {
                let items = projects
                    .iter()
                    .map(|p| {
                        let mut item = PaletteItem::new(project_name(p), PaletteAction::Command(Box::new(AddPackageTo { project: p.to_string_lossy().into_owned() })));
                        item.detail = p.parent().and_then(|d| d.strip_prefix(&root).ok()).map(|d| d.display().to_string()).filter(|d| !d.is_empty());
                        item
                    })
                    .collect();
                self.open_pick_palette(items, "Add a NuGet package to which project?", window, cx);
            }
        }
    }

    /// Opens the palette as a NuGet search; picking a result adds it to `project`.
    pub(super) fn open_package_search(&mut self, project: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_open = None;
        let placeholder = format!("Search NuGet for a package to add to {}", project_name(&project));
        self.package_target = Some(project);
        let items = vec![note("Type a package name to search nuget.org and the feeds in NuGet.config")];
        let palette = cx.new(|cx| Palette::new(Mode::Packages, items, &placeholder, "", window, cx));
        let sub = cx.subscribe_in(&palette, window, Self::on_palette_event);
        palette.update(cx, |p, cx| p.focus_input(window, cx));
        self.palette = Some((palette, sub));
        cx.notify();
    }

    /// Searches as the user types; a newer query replaces (and so cancels) the pending one.
    pub(super) fn search_packages(&mut self, query: String, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.root() else { return };
        if query.trim().is_empty() {
            self.package_search = None;
            return self.set_package_items(vec![note("Type a package name to search nuget.org and the feeds in NuGet.config")], cx);
        }
        self.package_search = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(350)).await;
            let _ = this.update(cx, |wb, cx| wb.set_package_items(vec![note(format!("Searching for “{query}”…"))], cx));
            let q = query.clone();
            // Stable versions first; a package that only has prereleases is found with them.
            let result = cx
                .background_executor()
                .spawn(async move {
                    match bf_core::nuget::search(&root, &q, false) {
                        Ok(found) if found.is_empty() => bf_core::nuget::search(&root, &q, true),
                        other => other,
                    }
                })
                .await;
            let _ = this.update(cx, |wb, cx| {
                let items = match result {
                    Ok(packages) if packages.is_empty() => vec![note(format!("No packages found for “{query}”"))],
                    Ok(packages) => packages
                        .into_iter()
                        .map(|p| {
                            let mut item = PaletteItem::new(p.id.clone(), PaletteAction::Package(p.id, p.version.clone()));
                            let downloads = format!("{} downloads", bf_core::nuget::format_downloads(p.downloads));
                            item.detail = Some(if p.owners.is_empty() { downloads } else { format!("{} · {downloads}", p.owners) });
                            item.hint = Some(if bf_core::nuget::is_prerelease(&p.version) { format!("{} (prerelease)", p.version) } else { p.version });
                            item
                        })
                        .collect(),
                    Err(e) => vec![note(format!("Search failed: {e:#}"))],
                };
                wb.set_package_items(items, cx);
            });
        }));
    }

    /// After a search pick: the package's versions (latest stable first, so Enter takes it; prereleases
    /// marked). Picking one adds it.
    pub(super) fn open_version_picker(&mut self, project: PathBuf, id: String, installed: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.root() else { return };
        let package = id.clone();
        let task = cx.background_spawn(async move { bf_core::nuget::versions(&root, &package) });
        cx.spawn_in(window, async move |this, cx| {
            let versions = task.await;
            let _ = this.update_in(cx, |wb, window, cx| {
                let versions = match versions {
                    Ok(v) if !v.is_empty() => v,
                    Ok(_) => return wb.add_package(project, id, None, window, cx),
                    Err(e) => {
                        wb.log(format!("Could not list the versions of {id} ({e:#}); adding the latest."));
                        return wb.add_package(project, id, None, window, cx);
                    }
                };
                let project_arg = project.to_string_lossy().into_owned();
                let items = versions
                    .iter()
                    .enumerate()
                    .map(|(i, v)| {
                        let mut item = PaletteItem::new(v.clone(), PaletteAction::Command(Box::new(AddPackageVersion { project: project_arg.clone(), id: id.clone(), version: v.clone() })));
                        let mut hint = match (i, bf_core::nuget::is_prerelease(v)) {
                            (0, false) => "latest stable".to_owned(),
                            (_, true) => "prerelease".to_owned(),
                            _ => String::new(),
                        };
                        if installed.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(v)) {
                            hint = if hint.is_empty() { "installed".into() } else { format!("{hint}, installed") };
                        }
                        item.hint = Some(hint);
                        item
                    })
                    .collect();
                let verb = if installed.is_some() { "use in" } else { "add to" };
                wb.open_pick_palette(items, &format!("Version of {id} to {verb} {}", project_name(&project)), window, cx);
            });
        })
        .detach();
    }

    fn set_package_items(&mut self, items: Vec<PaletteItem>, cx: &mut Context<Self>) {
        if let Some((palette, _)) = &self.palette
            && palette.read(cx).mode == Mode::Packages
        {
            palette.update(cx, |p, cx| p.set_items(Mode::Packages, items, cx));
        }
    }

    /// `dotnet add <project> package <id> [--version <v>]`: adds a package or changes its version.
    pub(super) fn add_package(&mut self, project: PathBuf, id: String, version: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let mut args = vec!["add".into(), project.to_string_lossy().into_owned(), "package".into(), id.clone()];
        if let Some(v) = version {
            args.extend(["--version".into(), v]);
        }
        self.package_command(project, id, args, window, cx);
    }

    pub(super) fn remove_package(&mut self, project: PathBuf, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let message = format!("Remove the package {id} from {}?", project_name(&project));
        let rx = window.prompt(PromptLevel::Warning, &message, None, &["Remove", "Cancel"], cx);
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(0) = rx.await {
                let _ = this.update_in(cx, |wb, window, cx| {
                    let args = vec!["remove".into(), project.to_string_lossy().into_owned(), "package".into(), id.clone()];
                    wb.package_command(project, id, args, window, cx);
                });
            }
        })
        .detach();
    }

    fn package_command(&mut self, project: PathBuf, id: String, args: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.root() else { return };
        if self.dotnet_busy(cx) {
            return;
        }
        // The project file is rewritten; unsaved edits in it would conflict.
        self.save_all(window, cx);
        self.show_panel(PanelView::Output, cx);
        let then: DotnetDone = Box::new(move |wb, ok, _, cx| {
            wb.solution.update(cx, |s, cx| {
                if ok {
                    s.package_changed(&project, &id);
                }
                s.load(cx);
            });
        });
        self.start_dotnet(root, args, Some(then), window, cx);
    }
}

impl Workbench {
    /// "Enable Central Package Management": writes `Directory.Packages.props` with every package's
    /// version (the highest where projects differ) and removes the versions from the projects.
    pub(super) fn enable_cpm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(props) = self.solution.read(cx).cpm().map(|c| c.props.clone()) else { return };
        let projects: Vec<(PathBuf, String)> = self.solution.read(cx).project_paths().into_iter().filter_map(|p| Some((p.clone(), std::fs::read_to_string(&p).ok()?))).collect();
        let props_text = std::fs::read_to_string(&props).ok();
        let plan = match bf_core::cpm::enable(&props, props_text.as_deref(), &projects) {
            Ok(plan) => plan,
            Err(e) => {
                self.log(format!("Could not enable Central Package Management: {e:#}"));
                self.show_panel(PanelView::Output, cx);
                return;
            }
        };
        let changed = plan.writes.len().saturating_sub(1);
        let mut detail = format!("{} gets every package's version; {changed} project file{} lose{} their own.", props.display(), if changed == 1 { "" } else { "s" }, if changed == 1 { "s" } else { "" });
        if !plan.notes.is_empty() {
            detail.push_str("\n\nDifferent versions become the highest:\n");
            detail.push_str(&plan.notes.join("\n"));
        }
        let rx = window.prompt(PromptLevel::Info, "Enable Central Package Management?", Some(&detail), &["Enable", "Cancel"], cx);
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(0) = rx.await {
                let _ = this.update_in(cx, |wb, window, cx| {
                    // Unsaved edits in a project file would be overwritten.
                    wb.save_all(window, cx);
                    wb.write_package_files(&plan.writes, cx);
                    for note in &plan.notes {
                        wb.log(format!("Central Package Management: {note}"));
                    }
                    wb.log(format!("Central Package Management is on: {}.", props.display()));
                });
            }
        })
        .detach();
    }

    /// "Move to Directory.Packages.props": the project's own version of `id` goes to the props file (unless
    /// that has one already, which then applies) and leaves the project.
    pub(super) fn move_to_central(&mut self, project: PathBuf, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(props) = self.solution.read(cx).cpm().map(|c| c.props.clone()) else { return };
        self.save_all(window, cx);
        let props_text = std::fs::read_to_string(&props).ok();
        let result = std::fs::read_to_string(&project).map_err(anyhow::Error::from).and_then(|text| bf_core::cpm::move_to_props(props_text.as_deref(), &text, &id));
        match result {
            Ok((new_props, new_project)) => {
                let before = props_text.as_deref().map(bf_core::cpm::central).unwrap_or_default();
                let mut writes = vec![(project.clone(), new_project)];
                if props_text.as_deref() != Some(new_props.as_str()) {
                    writes.insert(0, (props.clone(), new_props));
                }
                self.write_package_files(&writes, cx);
                match before.iter().find(|c| c.id.eq_ignore_ascii_case(&id)) {
                    Some(c) => self.log(format!("{} now uses {id} {} from {}.", project_name(&project), c.version, bf_core::cpm::PROPS)),
                    None => self.log(format!("Moved {id}'s version from {} to {}.", project_name(&project), bf_core::cpm::PROPS)),
                }
            }
            Err(e) => {
                self.log(format!("Could not move {id} to {}: {e:#}", bf_core::cpm::PROPS));
                self.show_panel(PanelView::Output, cx);
            }
        }
    }

    /// Writes project and props files, then reloads the editors showing them and the Solution view.
    fn write_package_files(&mut self, writes: &[(PathBuf, String)], cx: &mut Context<Self>) {
        for (path, text) in writes {
            if let Err(e) = std::fs::write(path, text) {
                self.log(format!("Could not write {}: {e}", path.display()));
                self.show_panel(PanelView::Output, cx);
            }
        }
        self.reload_changed_tabs(cx);
        self.solution.update(cx, |s, cx| s.load(cx));
        cx.notify();
    }
}

/// The nearest `.csproj` in the file's folder or a parent folder (up to the workspace root).
pub(super) fn owning_project(file: &Path, root: &Path) -> Option<PathBuf> {
    if file.extension().is_some_and(|e| e == "csproj") {
        return Some(file.to_path_buf());
    }
    let mut dir = file.parent();
    while let Some(d) = dir.filter(|d| d.starts_with(root)) {
        if let Some(project) = std::fs::read_dir(d).ok()?.flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|e| e == "csproj")) {
            return Some(project);
        }
        if d == root {
            break;
        }
        dir = d.parent();
    }
    None
}

#[cfg(test)]
mod cpm_tests {
    use crate::workbench::SideView;
    use crate::workbench::settings_page::view_tests::workbench;

    const API: &str = "<Project Sdk=\"Microsoft.NET.Sdk.Web\">\n  <ItemGroup>\n    <PackageReference Include=\"Serilog\" Version=\"4.0.0\" />\n    <PackageReference Include=\"Dapper\" Version=\"2.1.35\" />\n  </ItemGroup>\n</Project>\n";
    const WORKER: &str = "<Project Sdk=\"Microsoft.NET.Sdk.Worker\">\n  <ItemGroup>\n    <PackageReference Include=\"Serilog\" Version=\"3.1.1\" />\n  </ItemGroup>\n</Project>\n";

    /// The Solution view's NuGet node turns Central Package Management on (props file written, versions
    /// gone from the projects) and moves a version a project still carries into the props file.
    #[gpui::test]
    fn enables_central_package_management_and_moves_versions(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "cpm");
        let root = guard.dir.join("shop");
        for (name, text) in [("Api", API), ("Worker", WORKER)] {
            std::fs::create_dir_all(root.join(name)).unwrap();
            std::fs::write(root.join(format!("{name}/{name}.csproj")), text).unwrap();
        }
        let load = |cx: &mut gpui::VisualTestContext| {
            wb.update(cx, |wb, cx| wb.solution.update(cx, |s, cx| s.load(cx)));
            for _ in 0..200 {
                cx.run_until_parked();
                if wb.update(cx, |wb, cx| wb.solution.read(cx).cpm().is_some()) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        wb.update_in(cx, |wb, window, cx| {
            wb.open_folder(root.clone(), window, cx);
            wb.toggle_side(SideView::Solution, window, cx);
        });
        load(cx);
        assert!(!wb.update(cx, |wb, cx| wb.solution.read(cx).cpm().unwrap().enabled));
        // Open the NuGet node and click "Enable Central Package Management".
        let node = cx.debug_bounds("solution-nuget").expect("the NuGet node is drawn");
        cx.simulate_click(node.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        let enable = cx.debug_bounds("solution-enable-cpm").expect("the enable row is drawn");
        cx.simulate_click(enable.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        assert!(cx.has_pending_prompt(), "asks first");
        cx.simulate_prompt_answer("Enable");
        cx.run_until_parked();
        let props = std::fs::read_to_string(root.join("Directory.Packages.props")).unwrap();
        assert!(bf_core::cpm::enabled(&props));
        assert_eq!(bf_core::cpm::central(&props).iter().map(|c| (c.id.as_str(), c.version.as_str())).collect::<Vec<_>>(), [("Dapper", "2.1.35"), ("Serilog", "4.0.0")]);
        let api = std::fs::read_to_string(root.join("Api/Api.csproj")).unwrap();
        assert!(api.contains("<PackageReference Include=\"Serilog\" />"), "{api}");

        // A version added back by hand shows up with "Move to Directory.Packages.props".
        std::fs::write(root.join("Worker/Worker.csproj"), WORKER.replace("Serilog\" Version=\"3.1.1\"", "Polly\" Version=\"8.4.0\"")).unwrap();
        wb.update(cx, |wb, cx| wb.solution.update(cx, |s, _| s.forget_cpm_for_tests()));
        load(cx);
        cx.run_until_parked();
        assert!(cx.debug_bounds("solution-own-Polly").is_some(), "the own version is listed");
        let worker = root.join("Worker/Worker.csproj");
        wb.update_in(cx, |wb, window, cx| wb.move_to_central(worker.clone(), "Polly".into(), window, cx));
        let props = std::fs::read_to_string(root.join("Directory.Packages.props")).unwrap();
        assert!(bf_core::cpm::central(&props).iter().any(|c| c.id == "Polly" && c.version == "8.4.0"), "{props}");
        assert!(std::fs::read_to_string(&worker).unwrap().contains("<PackageReference Include=\"Polly\" />"));
        drop(guard);
    }
}
