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
