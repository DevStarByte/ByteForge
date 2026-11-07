//! Docker in the workbench: the Docker view's events, building images and running Compose in terminals,
//! and "Add Docker Support" / "Add Docker Compose File" for .NET projects.

use std::collections::HashSet;

use bf_core::docker::{self as dk, ComposeService, DotnetProject};

use super::*;
use crate::docker::{ComposeVerb, DockerEvent, DockerView};

impl Workbench {
    pub(super) fn on_docker_event(&mut self, _: &Entity<DockerView>, event: &DockerEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            DockerEvent::OpenFile(path) => self.open_file(path.clone(), None, window, cx),
            DockerEvent::RunInTerminal { title, args } => {
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                self.run_in_terminal(title.clone(), dk::command_line(&args), window, cx);
            }
            DockerEvent::Build(path) => self.docker_build(path.clone(), window, cx),
            DockerEvent::Compose(path, verb) => self.docker_compose(path.clone(), *verb == ComposeVerb::Up, window, cx),
            DockerEvent::Log(text) => self.log(text),
            DockerEvent::StatusChanged => cx.notify(),
        }
    }

    /// Opens a new terminal named `title` and runs `command` in the user's shell.
    fn run_in_terminal(&mut self, title: String, command: String, window: &mut Window, cx: &mut Context<Self>) {
        let before = self.terminals.len();
        self.new_terminal(window, cx);
        if self.terminals.len() == before {
            return;
        }
        if let Some((terminal, _)) = self.terminals.last() {
            terminal.update(cx, |t, cx| {
                t.label = Some(title);
                t.run_command(&command, cx);
            });
        }
        cx.notify();
    }

    /// `docker build` of a Dockerfile in a terminal, tagged after its folder, from its build context.
    pub(super) fn docker_build(&mut self, dockerfile: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.save_all(window, cx);
        let root = self.workspace.root().or_else(|| dockerfile.parent().map(Path::to_path_buf)).unwrap_or_default();
        let context = dk::build_context(&dockerfile, &root);
        let tag = dk::image_name_for(&dockerfile);
        let file = dockerfile.to_string_lossy().into_owned();
        let ctx = context.to_string_lossy().into_owned();
        let command = dk::command_line(&["docker", "build", "--file", &file, "--tag", &tag, &ctx]);
        self.log(format!("> {command}"));
        self.run_in_terminal(format!("build: {tag}"), command, window, cx);
    }

    /// `docker compose up --build --detach` (or `down`) for a Compose file in a terminal. The command is
    /// `"docker.composeCommand"` (default `docker compose`; set `docker-compose` for the standalone tool).
    pub(super) fn docker_compose(&mut self, file: PathBuf, up: bool, window: &mut Window, cx: &mut Context<Self>) {
        if up {
            self.save_all(window, cx);
        }
        let compose = self.settings.get_string("docker.composeCommand").filter(|c| !c.trim().is_empty()).unwrap_or_else(|| "docker compose".into());
        let path = file.to_string_lossy().into_owned();
        let mut args: Vec<&str> = compose.split_whitespace().collect();
        args.extend(["--file", &path]);
        if up {
            args.extend(["up", "--build", "--detach"]);
        } else {
            args.push("down");
        }
        let command = dk::command_line(&args);
        self.log(format!("> {command}"));
        let name = file.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.run_in_terminal(format!("compose {}: {name}", if up { "up" } else { "down" }), command, window, cx);
        // The containers show up (or go away) in the Docker view a moment later.
        self.show_side(SideView::Docker, cx);
    }

    /// Build (`compose: false`) or Compose up/down (`compose_up: Some(up)`): the active editor's file when it
    /// is one, the only one in the workspace, or one picked from a list.
    pub(super) fn docker_pick_file(&mut self, compose: bool, compose_up: Option<bool>, window: &mut Window, cx: &mut Context<Self>) {
        let up = compose_up.unwrap_or(true);
        let matches = |path: &Path| {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if compose { dk::is_compose_file_name(&name) } else { bf_syntax::is_dockerfile_name(&name) }
        };
        let active = self.active.and_then(|i| self.tabs.get(i)).and_then(|t| t.path.clone()).filter(|p| matches(p));
        let run = |wb: &mut Self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>| {
            if compose { wb.docker_compose(path, up, window, cx) } else { wb.docker_build(path, window, cx) }
        };
        if let Some(path) = active {
            run(self, path, window, cx);
            return;
        }
        let (dockerfiles, compose_files) = dk::docker_files(&self.workspace.files(), bf_syntax::is_dockerfile_name);
        let files = if compose { compose_files } else { dockerfiles };
        match files.len() {
            0 => {
                self.log(if compose {
                    "No Compose file (compose.yaml, docker-compose.yml) in the workspace. Create one with \"Docker: Add Docker Compose File\"."
                } else {
                    "No Dockerfile in the workspace. Create one with \"Docker: Add Docker Support to Project…\"."
                });
                self.show_panel(PanelView::Output, cx);
            }
            1 => run(self, files.into_iter().next().unwrap_or_default(), window, cx),
            _ => {
                let root = self.workspace.root().unwrap_or_default();
                let items = files
                    .into_iter()
                    .map(|path| {
                        let label = path.strip_prefix(&root).unwrap_or(&path).display().to_string();
                        let action: Box<dyn gpui::Action> = if compose { Box::new(DockerComposeFile { path, up }) } else { Box::new(DockerBuildFile { path }) };
                        PaletteItem::new(label, PaletteAction::Command(action))
                    })
                    .collect();
                let placeholder = match (compose, up) {
                    (false, _) => "Select the Dockerfile to build",
                    (true, true) => "Select the Compose file to start",
                    (true, false) => "Select the Compose file to stop",
                };
                self.open_pick_palette(items, placeholder, window, cx);
            }
        }
    }

    fn open_pick_palette(&mut self, items: Vec<PaletteItem>, placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_open = None;
        let palette = cx.new(|cx| Palette::new(Mode::Pick, items, placeholder, "", window, cx));
        let sub = cx.subscribe_in(&palette, window, Self::on_palette_event);
        palette.update(cx, |p, cx| p.focus_input(window, cx));
        self.palette = Some((palette, sub));
        cx.notify();
    }

    /// Picks the .NET project to add a Dockerfile to (apps first), or uses the only one.
    pub(super) fn docker_add_support(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.root() else {
            self.log("Open a folder containing a .NET project first.");
            self.show_panel(PanelView::Output, cx);
            return;
        };
        let mut projects = bf_core::dotnet::projects(&root);
        if projects.is_empty() {
            self.log(format!("No .csproj found in {}.", root.display()));
            self.show_panel(PanelView::Output, cx);
            return;
        }
        if projects.len() == 1 {
            self.docker_add_support_to(projects.remove(0), window, cx);
            return;
        }
        let mut items: Vec<(bool, PaletteItem)> = projects
            .into_iter()
            .map(|path| {
                let app = bf_core::dotnet::is_app(&path);
                let web = DotnetProject::read(&path).is_ok_and(|p| p.web);
                let has_dockerfile = path.parent().is_some_and(|d| d.join("Dockerfile").exists());
                let label = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let mut item = PaletteItem::new(label, PaletteAction::Command(Box::new(DockerAddSupportTo { path: path.clone() })));
                item.detail = path.parent().and_then(|p| p.strip_prefix(&root).ok()).map(|p| p.display().to_string()).filter(|s| !s.is_empty());
                let kind = if web { "web app" } else if app { "app" } else { "library" };
                item.hint = Some(if has_dockerfile { format!("{kind} · has a Dockerfile") } else { kind.to_owned() });
                (app, item)
            })
            .collect();
        items.sort_by_key(|(app, _)| !app);
        self.open_pick_palette(items.into_iter().map(|(_, item)| item).collect(), "Select the project to add Docker support to", window, cx);
    }

    /// Writes a Dockerfile next to `project` (asking before replacing one) and a `.dockerignore` in the
    /// build context: the workspace folder for solutions and projects that reference others, else the
    /// project's folder.
    pub(super) fn docker_add_support_to(&mut self, project: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let info = match DotnetProject::read(&project) {
            Ok(info) => info,
            Err(e) => {
                self.log(format!("Could not read {}: {e}", project.display()));
                return;
            }
        };
        let Some(project_dir) = project.parent().map(Path::to_path_buf) else { return };
        let root = self.workspace.root().filter(|r| project.starts_with(r)).unwrap_or_else(|| project_dir.clone());
        let has_solution = std::fs::read_dir(&root).is_ok_and(|entries| {
            entries.flatten().any(|e| matches!(e.path().extension().and_then(|x| x.to_str()), Some("sln" | "slnx")))
        });
        let context = if has_solution || !info.references().is_empty() { root } else { project_dir.clone() };
        let dockerfile = project_dir.join("Dockerfile");
        let text = dk::dotnet_dockerfile(&info, &context);
        let write = move |wb: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
            let ignore = context.join(".dockerignore");
            let result = std::fs::write(&dockerfile, &text).and_then(|_| if ignore.exists() { Ok(()) } else { std::fs::write(&ignore, dk::DOTNET_DOCKERIGNORE) });
            match result {
                Ok(()) => {
                    wb.log(format!(
                        "Added {} ({} on .NET {}, build context {}). Build it with \"Docker: Build Image…\".",
                        dockerfile.display(),
                        if info.web { "ASP.NET Core" } else { "console app" },
                        info.version,
                        context.display()
                    ));
                    wb.explorer.update(cx, |e, cx| e.refresh(cx));
                    wb.docker.update(cx, |d, cx| d.refresh(cx));
                    wb.open_file(dockerfile.clone(), None, window, cx);
                }
                Err(e) => {
                    wb.log(format!("Could not write {}: {e}", dockerfile.display()));
                    wb.show_panel(PanelView::Output, cx);
                }
            }
        };
        if !project_dir.join("Dockerfile").exists() {
            write(self, window, cx);
            return;
        }
        let rx = window.prompt(PromptLevel::Warning, "This project already has a Dockerfile.", Some("Replace it with a generated one?"), &["Replace", "Cancel"], cx);
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(0) = rx.await {
                let _ = this.update_in(cx, |wb, window, cx| write(wb, window, cx));
            }
        })
        .detach();
    }

    /// Writes a `compose.yaml` in the workspace folder with a service per Dockerfile, or opens the existing one.
    pub(super) fn docker_add_compose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.root() else {
            self.log("Open a folder first.");
            self.show_panel(PanelView::Output, cx);
            return;
        };
        let (dockerfiles, compose_files) = dk::docker_files(&self.workspace.files(), bf_syntax::is_dockerfile_name);
        if let Some(existing) = compose_files.iter().find(|p| p.parent() == Some(root.as_path())) {
            self.log(format!("{} already exists.", existing.display()));
            self.open_file(existing.clone(), None, window, cx);
            return;
        }
        if dockerfiles.is_empty() {
            self.log("The workspace has no Dockerfile yet. Run \"Docker: Add Docker Support to Project…\" first.");
            self.show_panel(PanelView::Output, cx);
            return;
        }
        let mut names = HashSet::new();
        let services: Vec<ComposeService> = dockerfiles
            .iter()
            .map(|dockerfile| {
                let text = std::fs::read_to_string(dockerfile).unwrap_or_default();
                let mut name = dk::image_name_for(dockerfile);
                let mut n = 2;
                while !names.insert(name.clone()) {
                    name = format!("{}-{n}", dk::image_name_for(dockerfile));
                    n += 1;
                }
                ComposeService { name, dockerfile: dockerfile.clone(), context: dk::build_context(dockerfile, &root), web: text.contains("EXPOSE") || text.contains("dotnet/aspnet") }
            })
            .collect();
        let path = root.join("compose.yaml");
        match std::fs::write(&path, dk::compose_file(&services, &root)) {
            Ok(()) => {
                self.log(format!("Added {} with {} service(s). Start it with \"Docker: Compose Up\".", path.display(), services.len()));
                self.explorer.update(cx, |e, cx| e.refresh(cx));
                self.docker.update(cx, |d, cx| d.refresh(cx));
                self.open_file(path, None, window, cx);
            }
            Err(e) => {
                self.log(format!("Could not write {}: {e}", path.display()));
                self.show_panel(PanelView::Output, cx);
            }
        }
    }
}
