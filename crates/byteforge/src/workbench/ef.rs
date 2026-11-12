//! EF Core migrations: Add Migration, Remove Last Migration, Update Database (to the latest or a picked
//! migration), Generate SQL Script and Drop Database, run with `dotnet ef`.
//!
//! The `DbContext` comes from the source (asked when there are several), the startup project is the
//! context's own project when it is an app, else the startup project (its configuration holds the
//! connection string).

use bf_core::ef::{self, Op};

use super::*;
use crate::actions::EfCommand;

impl Workbench {
    /// A migrations command from the palette or a menu: `op` is add, remove, update, update-to, script or drop.
    pub(super) fn start_ef(&mut self, op: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.run_ef(EfCommand { op: op.into(), project: String::new(), context: String::new(), arg: String::new() }, window, cx);
    }

    /// Fills in what `cmd` still lacks (the context, a name, a target migration, a confirmation), asking
    /// for it, then runs it.
    pub(super) fn run_ef(&mut self, mut cmd: EfCommand, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.root() else {
            self.log("Open a folder with a .NET project first.");
            self.show_panel(PanelView::Output, cx);
            return;
        };
        if cmd.project.is_empty() {
            let task = cx.background_spawn(async move { ef::find_contexts(&root) });
            cx.spawn_in(window, async move |this, cx| {
                let contexts = task.await;
                let _ = this.update_in(cx, |wb, window, cx| wb.pick_context(cmd, contexts, window, cx));
            })
            .detach();
            return;
        }
        match (cmd.op.as_str(), cmd.arg.is_empty()) {
            ("add", true) => self.open_prompt(PromptKind::EfMigrationName(cmd), "", window, cx),
            ("update-to", true) => self.pick_migration(cmd, window, cx),
            ("remove" | "drop", true) => {
                let (question, detail, button) = if cmd.op == "remove" {
                    ("Remove the last migration?", "Its files are deleted and the model snapshot goes back. EF checks the database first, so it has to be reachable; a migration that is already applied can't be removed (update the database to the one before it first).", "Remove")
                } else {
                    ("Drop the database?", "The database the startup project's connection string points to is deleted with all its data.", "Drop Database")
                };
                let rx = window.prompt(PromptLevel::Warning, question, Some(detail), &[button, "Cancel"], cx);
                cx.spawn_in(window, async move |this, cx| {
                    if let Ok(0) = rx.await {
                        cmd.arg = "confirmed".into();
                        let _ = this.update_in(cx, |wb, window, cx| wb.execute_ef(cmd, window, cx));
                    }
                })
                .detach();
            }
            _ => self.execute_ef(cmd, window, cx),
        }
    }

    fn pick_context(&mut self, cmd: EfCommand, contexts: Vec<ef::DbContextInfo>, window: &mut Window, cx: &mut Context<Self>) {
        let with = |c: &ef::DbContextInfo| EfCommand { project: c.project.to_string_lossy().into_owned(), context: c.name.clone(), ..cmd.clone() };
        match contexts.as_slice() {
            [] => {
                self.log("EF Core: no DbContext found. Migrations need a class deriving from DbContext in one of the projects.");
                self.show_panel(PanelView::Output, cx);
            }
            [only] => self.run_ef(with(only), window, cx),
            many => {
                let items = many
                    .iter()
                    .map(|c| {
                        let mut item = PaletteItem::new(c.name.clone(), PaletteAction::Command(Box::new(with(c))));
                        item.detail = c.project.file_stem().map(|s| s.to_string_lossy().into_owned());
                        item
                    })
                    .collect();
                self.show_pick(items, "DbContext to use", window, cx);
            }
        }
    }

    /// The startup project `dotnet ef` runs through for `project`: itself when it is an app, else the
    /// folder's startup project (its configuration has the connection string).
    fn ef_startup(&self, project: &Path) -> PathBuf {
        if bf_core::dotnet::is_app(project) { project.to_path_buf() } else { self.startup_project().unwrap_or_else(|| project.to_path_buf()) }
    }

    /// "Update Database to Migration…": the migrations from the source at once (newest first); which are
    /// applied is filled in when EF has asked the database (it builds first, so that takes a moment).
    fn pick_migration(&mut self, cmd: EfCommand, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.root() else { return };
        let project = PathBuf::from(&cmd.project);
        let startup = self.ef_startup(&project);
        let context = (!cmd.context.is_empty()).then(|| cmd.context.clone());
        let scan = {
            let (project, context) = (project.clone(), context.clone());
            cx.background_spawn(async move { ef::migrations(&project, context.as_deref()) })
        };
        cx.spawn_in(window, async move |this, cx| {
            let found = scan.await;
            let palette = this.update_in(cx, |wb, window, cx| {
                wb.show_pick(migration_items(&found, &cmd, &[]), "Update the database to migration (newest first; asking the database which are applied…)", window, cx);
                wb.palette.as_ref().map(|(p, _)| p.clone())
            });
            let Ok(Some(palette)) = palette else { return };
            let context = context.unwrap_or_default();
            let listed = cx.background_executor().spawn(async move { ef::list(&root, &project, &startup, &context) }).await;
            let _ = this.update(cx, |wb, cx| {
                // Only while that picker is still the one open.
                if wb.palette.as_ref().is_none_or(|(p, _)| *p != palette) {
                    return;
                }
                match listed {
                    Ok((listed, warning)) => {
                        if let Some(w) = warning {
                            wb.log(format!("EF Core: the database couldn't be asked which migrations are applied: {w}"));
                        }
                        palette.update(cx, |p, cx| p.set_items(Mode::Pick, migration_items(&found, &cmd, &listed), cx));
                    }
                    Err(e) => wb.log(format!("EF Core: could not ask which migrations are applied: {e:#}")),
                }
            });
        })
        .detach();
    }

    /// "Scaffold DbContext from Database…": asks for the connection string, then the provider, and
    /// generates the entity classes and the DbContext into Models/ of the current file's project (else
    /// the startup project).
    pub(super) fn start_scaffold(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.root() else {
            self.log("Open a folder with a .NET project first.");
            return;
        };
        let active = self.active.and_then(|ix| self.tabs.get(ix)).and_then(|t| t.path.clone());
        let Some(project) = active.and_then(|p| super::nuget::owning_project(&p, &root)).or_else(|| self.startup_project()) else {
            self.log("EF Core: no .csproj found. Open a file of the project to scaffold into.");
            self.show_panel(PanelView::Output, cx);
            return;
        };
        self.open_prompt(PromptKind::EfScaffoldConnection { project }, "", window, cx);
    }

    /// The connection string was typed: which provider?
    pub(super) fn scaffold_pick_provider(&mut self, project: PathBuf, connection: String, window: &mut Window, cx: &mut Context<Self>) {
        if connection.trim().is_empty() {
            return;
        }
        let project_arg = project.to_string_lossy().into_owned();
        let items = ef::PROVIDERS
            .iter()
            .map(|(name, package)| {
                // For scaffolding, `context` carries the provider package and `arg` the connection string.
                let cmd = EfCommand { op: "scaffold".into(), project: project_arg.clone(), context: (*package).to_owned(), arg: connection.trim().to_owned() };
                let mut item = PaletteItem::new(*name, PaletteAction::Command(Box::new(cmd)));
                item.detail = Some((*package).to_owned());
                item
            })
            .collect();
        let name = project.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.show_pick(items, &format!("Database provider (its package must be referenced by {name})"), window, cx);
    }

    fn show_pick(&mut self, items: Vec<PaletteItem>, placeholder: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_open = None;
        let palette = cx.new(|cx| Palette::new(Mode::Pick, items, placeholder, "", window, cx));
        let sub = cx.subscribe_in(&palette, window, Self::on_palette_event);
        palette.update(cx, |p, cx| p.focus_input(window, cx));
        self.palette = Some((palette, sub));
        cx.notify();
    }

    /// The migration name was typed (Add Migration).
    pub(super) fn ef_migration_named(&mut self, cmd: EfCommand, typed: &str, window: &mut Window, cx: &mut Context<Self>) {
        match ef::migration_name(typed) {
            Some(name) => self.run_ef(EfCommand { arg: name, ..cmd }, window, cx),
            None => self.log("EF Core: a migration needs a name (letters and digits, like AddOrders)."),
        }
    }

    fn execute_ef(&mut self, cmd: EfCommand, window: &mut Window, cx: &mut Context<Self>) {
        if self.dotnet_busy(cx) {
            return;
        }
        let Some(root) = self.workspace.root() else { return };
        let project = PathBuf::from(&cmd.project);
        let startup = self.ef_startup(&project);
        if let Some(unrestored) = [&startup, &project].into_iter().find(|p| ef::needs_restore(p)) {
            // Restore first (the startup project's restore covers the projects it references), then go on.
            let args = vec!["restore".into(), unrestored.to_string_lossy().into_owned()];
            let then: DotnetDone = Box::new(move |wb, ok, window, cx| {
                if ok {
                    wb.execute_ef(cmd, window, cx);
                } else {
                    wb.show_panel(PanelView::Output, cx);
                }
            });
            self.show_panel(PanelView::Output, cx);
            self.start_dotnet(root, args, Some(then), window, cx);
            return;
        }
        let op = match cmd.op.as_str() {
            "add" => Op::Add(cmd.arg.clone()),
            "remove" => Op::Remove,
            "update" => Op::Update(None),
            "update-to" => Op::Update(Some(cmd.arg.clone())),
            "drop" => Op::Drop,
            "scaffold" => Op::Scaffold { connection: cmd.arg.clone(), provider: cmd.context.clone(), output_dir: "Models".into() },
            "script" => {
                let dir = std::env::temp_dir().join("byteforge-ef");
                let _ = std::fs::create_dir_all(&dir);
                let stem = if cmd.context.is_empty() { project.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default() } else { cmd.context.clone() };
                Op::Script(dir.join(format!("{stem}-migrations.sql")))
            }
            other => {
                self.log(format!("EF Core: unknown command \"{other}\"."));
                return;
            }
        };
        // For scaffolding, `context` is the provider (the DbContext is what gets generated).
        let context_arg = if matches!(op, Op::Scaffold { .. }) { "" } else { cmd.context.as_str() };
        let args = ef::args(&op, &project, &startup, context_arg);
        let missing = std::rc::Rc::new(std::cell::Cell::new(false));
        let seen = missing.clone();
        let on_line: LineHook = Box::new(move |_, line, _, _| {
            if ef::is_tool_missing(line) {
                seen.set(true);
            }
        });
        let context = cmd.context.clone();
        let then: DotnetDone = Box::new(move |wb, ok, window, cx| {
            if !ok {
                if missing.get() {
                    wb.offer_ef_install(window, cx);
                } else {
                    wb.show_panel(PanelView::Output, cx);
                }
                return;
            }
            match op {
                Op::Add(name) => {
                    let latest = ef::migrations(&project, (!context.is_empty()).then_some(context.as_str())).into_iter().rev().find(|m| m.name == name);
                    if let Some(m) = latest {
                        wb.open_file(m.file, None, window, cx);
                    }
                    wb.log(format!("EF Core: added migration {name}. Apply it with \"EF Core: Update Database\"."));
                }
                Op::Script(file) => wb.open_file(file, None, window, cx),
                Op::Remove => wb.log("EF Core: removed the last migration."),
                Op::Update(None) => wb.log("EF Core: the database is up to date."),
                Op::Update(Some(target)) if target == "0" => wb.log("EF Core: reverted all migrations."),
                Op::Update(Some(target)) => wb.log(format!("EF Core: the database is at {target}.")),
                Op::Drop => wb.log("EF Core: dropped the database."),
                Op::Scaffold { output_dir, .. } => wb.log(format!("EF Core: scaffolded the entity classes and the DbContext into {output_dir}/. The connection string is in the DbContext's OnConfiguring: move it to configuration (user secrets) before committing.")),
            }
            wb.explorer.update(cx, |e, cx| e.refresh(cx));
        });
        self.show_panel(PanelView::Output, cx);
        self.start_dotnet_with(root, args, Vec::new(), Some(on_line), Some(then), window, cx);
    }

    /// `dotnet ef` isn't installed: offers to install it as a global tool.
    fn offer_ef_install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.log("EF Core: the dotnet-ef tool isn't installed (dotnet tool install --global dotnet-ef).");
        let rx = window.prompt(PromptLevel::Info, "The dotnet-ef tool isn't installed.", Some("Migrations run through it. Install it as a global .NET tool now?"), &["Install dotnet-ef", "Cancel"], cx);
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(0) = rx.await {
                let _ = this.update_in(cx, |wb, window, cx| {
                    let Some(root) = wb.workspace.root() else { return };
                    let args = vec!["tool".into(), "install".into(), "--global".into(), "dotnet-ef".into()];
                    let then: DotnetDone = Box::new(|wb, ok, _, cx| {
                        wb.log(if ok { "dotnet-ef is installed. Run the migrations command again." } else { "Installing dotnet-ef failed; see the output above." });
                        wb.show_panel(PanelView::Output, cx);
                    });
                    wb.start_dotnet(root, args, Some(then), window, cx);
                });
            }
        })
        .detach();
    }
}

/// The picker's rows for "Update Database to Migration…": newest first, with "applied" or "pending" when
/// EF reported it, then "Revert all migrations".
fn migration_items(found: &[ef::Migration], cmd: &EfCommand, listed: &[ef::ListedMigration]) -> Vec<PaletteItem> {
    let with = |arg: &str| PaletteAction::Command(Box::new(EfCommand { arg: arg.to_owned(), ..cmd.clone() }));
    let mut items: Vec<PaletteItem> = found
        .iter()
        .rev()
        .map(|m| {
            let mut item = PaletteItem::new(m.name.clone(), with(&m.id));
            item.detail = Some(m.id.clone());
            item.hint = listed.iter().find(|l| l.id == m.id).and_then(|l| l.applied).map(|applied| if applied { "applied".to_owned() } else { "pending".to_owned() });
            item
        })
        .collect();
    let mut revert = PaletteItem::new("Revert all migrations", with("0"));
    revert.detail = Some("an empty database (dotnet ef database update 0)".into());
    items.push(revert);
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_picker_marks_applied_and_pending() {
        let m = |id: &str, name: &str| ef::Migration { id: id.into(), name: name.into(), context: None, file: PathBuf::from("x.cs") };
        let found = [m("1_Init", "Init"), m("2_AddOrders", "AddOrders")];
        let cmd = EfCommand { op: "update-to".into(), project: "/w/A.csproj".into(), context: String::new(), arg: String::new() };
        let before = migration_items(&found, &cmd, &[]);
        assert_eq!(before.iter().map(|i| (i.label.as_str(), i.hint.as_deref())).collect::<Vec<_>>(), [("AddOrders", None), ("Init", None), ("Revert all migrations", None)]);
        let listed = [ef::ListedMigration { id: "1_Init".into(), applied: Some(true) }, ef::ListedMigration { id: "2_AddOrders".into(), applied: Some(false) }];
        let after = migration_items(&found, &cmd, &listed);
        assert_eq!(after.iter().map(|i| i.hint.as_deref()).collect::<Vec<_>>(), [Some("pending"), Some("applied"), None]);
    }
}
