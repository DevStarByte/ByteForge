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

    fn pick_migration(&mut self, cmd: EfCommand, window: &mut Window, cx: &mut Context<Self>) {
        let project = PathBuf::from(&cmd.project);
        let context = (!cmd.context.is_empty()).then(|| cmd.context.clone());
        let task = cx.background_spawn(async move { ef::migrations(&project, context.as_deref()) });
        cx.spawn_in(window, async move |this, cx| {
            let found = task.await;
            let _ = this.update_in(cx, |wb, window, cx| {
                let with = |arg: &str| PaletteAction::Command(Box::new(EfCommand { arg: arg.to_owned(), ..cmd.clone() }));
                let mut items: Vec<PaletteItem> = found
                    .iter()
                    .rev()
                    .map(|m| {
                        let mut item = PaletteItem::new(m.name.clone(), with(&m.id));
                        item.detail = Some(m.id.clone());
                        item
                    })
                    .collect();
                let mut revert = PaletteItem::new("Revert all migrations", with("0"));
                revert.detail = Some("an empty database (dotnet ef database update 0)".into());
                items.push(revert);
                wb.show_pick(items, "Update the database to migration (newest first)", window, cx);
            });
        })
        .detach();
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
        let startup = if bf_core::dotnet::is_app(&project) { project.clone() } else { self.startup_project().unwrap_or_else(|| project.clone()) };
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
        let args = ef::args(&op, &project, &startup, &cmd.context);
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
