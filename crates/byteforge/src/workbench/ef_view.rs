//! The EF Core view: each project's `DbContext`s and their migrations, read from the source at once and
//! marked applied or pending once EF has asked the database. It runs the EF commands of the Run menu
//! (`workbench/ef.rs`) and refreshes after each one.

use std::collections::{HashMap, HashSet};

use bf_core::ef;

use super::*;
use crate::actions::EfCommand;

#[derive(Default)]
pub(super) struct EfViewState {
    contexts: Vec<EfContext>,
    pub(super) loaded: bool,
    loading: Option<Task<()>>,
    /// The database check (`dotnet ef migrations list`, which builds first: it takes a while).
    asking: Option<Task<()>>,
    /// Contexts the user closed, by key.
    collapsed: HashSet<String>,
    menu: Option<EfMenu>,
}

struct EfContext {
    info: ef::DbContextInfo,
    migrations: Vec<ef::Migration>,
    /// Migration id → applied, once the database was asked.
    applied: Option<HashMap<String, bool>>,
    /// Why the database couldn't be asked.
    note: Option<String>,
}

impl EfContext {
    fn key(&self) -> String {
        format!("{}|{}", self.info.project.display(), self.info.name)
    }
}

/// The right-click menu of a migration.
struct EfMenu {
    project: String,
    context: String,
    id: String,
    last: bool,
    position: gpui::Point<gpui::Pixels>,
}

impl Workbench {
    /// Reads the contexts and migrations from the source again (instant), keeping what the database said;
    /// `ask` also asks the database which migrations are applied.
    pub(super) fn refresh_ef_view(&mut self, ask: bool, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.root() else {
            self.ef_view.contexts.clear();
            self.ef_view.loaded = true;
            return;
        };
        self.ef_view.loading = Some(cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    ef::find_contexts(&root)
                        .into_iter()
                        .map(|info| {
                            let migrations = ef::migrations(&info.project, Some(&info.name));
                            (info, migrations)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |wb, cx| {
                let mut previous: HashMap<String, EfContext> = wb.ef_view.contexts.drain(..).map(|c| (c.key(), c)).collect();
                wb.ef_view.contexts = found
                    .into_iter()
                    .map(|(info, migrations)| {
                        let mut context = EfContext { info, migrations, applied: None, note: None };
                        if let Some(old) = previous.remove(&context.key()) {
                            context.applied = old.applied;
                            context.note = old.note;
                        }
                        context
                    })
                    .collect();
                wb.ef_view.loaded = true;
                wb.ef_view.loading = None;
                if ask {
                    wb.ask_ef_database(cx);
                }
                cx.notify();
            });
        }));
    }

    /// Asks the database, context by context, which migrations are applied.
    fn ask_ef_database(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.root() else { return };
        let jobs: Vec<(String, PathBuf, PathBuf, String)> = self
            .ef_view
            .contexts
            .iter()
            .filter(|c| !c.migrations.is_empty())
            .map(|c| (c.key(), c.info.project.clone(), self.ef_startup(&c.info.project), c.info.name.clone()))
            .collect();
        if jobs.is_empty() {
            return;
        }
        self.ef_view.asking = Some(cx.spawn(async move |this, cx| {
            for (key, project, startup, name) in jobs {
                let root = root.clone();
                let listed = cx.background_executor().spawn(async move { ef::list(&root, &project, &startup, &name) }).await;
                let alive = this.update(cx, |wb, cx| {
                    if let Some(c) = wb.ef_view.contexts.iter_mut().find(|c| c.key() == key) {
                        match listed {
                            Ok((listed, warning)) => {
                                c.applied = Some(listed.into_iter().filter_map(|m| Some((m.id, m.applied?))).collect());
                                c.note = warning.map(|w| format!("The database couldn't be asked: {w}"));
                            }
                            Err(e) => c.note = Some(format!("Could not ask which migrations are applied: {e:#}")),
                        }
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    return;
                }
            }
            let _ = this.update(cx, |wb, cx| {
                wb.ef_view.asking = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn toggle_ef_context(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.ef_view.collapsed.remove(&key) {
            self.ef_view.collapsed.insert(key);
        }
        cx.notify();
    }

    fn run_ef_from_view(&mut self, op: &str, project: String, context: String, arg: String, window: &mut Window, cx: &mut Context<Self>) {
        self.ef_view.menu = None;
        self.run_ef(EfCommand { op: op.into(), project, context, arg }, window, cx);
    }

    /// The EF Core side view.
    pub(super) fn render_ef_view(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = Theme::global(cx);
        let muted = t.c("foreground.muted");
        let row_h = t.n("list.itemHeight", 22.0);
        if self.workspace.root().is_none() {
            return div().p_3().text_size(px(12.)).text_color(muted).child("Open a folder with a .NET project to see its DbContexts and migrations.").into_any_element();
        }
        let small = |id: &'static str, name: &str, tip: &str| {
            div()
                .id(id)
                .debug_selector(move || id.to_owned())
                .p(px(2.))
                .rounded(px(3.))
                .cursor_pointer()
                .hover(|s| s.bg(t.c("toolbar.hoverBackground")))
                .tooltip(tooltip(tip.to_owned()))
                .child(icon(name, px(14.), t.c("icon.foreground")))
        };
        let state = &self.ef_view;
        let status = if !state.loaded {
            "Looking for DbContexts…".to_owned()
        } else if state.asking.is_some() {
            "Asking the database which migrations are applied…".to_owned()
        } else {
            match state.contexts.len() {
                0 => String::new(),
                1 => "1 DbContext".to_owned(),
                n => format!("{n} DbContexts"),
            }
        };
        let top = div()
            .px_3()
            .flex()
            .items_center()
            .gap_1()
            .min_h(px(22.))
            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_size(px(12.)).text_color(muted).child(status))
            .child(small("ef-add", "add", "Add Migration…").on_click(cx.listener(|wb, _, w, cx| wb.start_ef("add", w, cx))))
            .child(small("ef-update", "arrowUp", "Update Database").on_click(cx.listener(|wb, _, w, cx| wb.start_ef("update", w, cx))))
            .child(small("ef-drop", "trash", "Drop Database").on_click(cx.listener(|wb, _, w, cx| wb.start_ef("drop", w, cx))))
            .child(small("ef-scaffold", "pencil", "Scaffold DbContext from Database…").on_click(cx.listener(|wb, _, w, cx| wb.start_scaffold(w, cx))))
            .child(small("ef-refresh", "refresh", "Refresh (asks the database again)").on_click(cx.listener(|wb, _, _, cx| wb.refresh_ef_view(true, cx))));

        let mut list = div().id("ef-list").flex_1().min_h_0().overflow_y_scroll().flex().flex_col();
        if state.loaded && state.contexts.is_empty() {
            list = list.child(
                div()
                    .p_3()
                    .text_size(px(12.))
                    .text_color(muted)
                    .child("No DbContext found. Migrations need a class deriving from DbContext in one of the projects; Scaffold DbContext creates one from an existing database."),
            );
        }
        let (passed, pending) = (t.c_or("testing.iconPassed", 0x73c991ff), t.c_or("editorWarning.foreground", 0xcca700ff));
        for (ci, c) in state.contexts.iter().enumerate() {
            let key = c.key();
            let open = !state.collapsed.contains(&key);
            let project = c.info.project.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            list = list.child(
                div()
                    .id(SharedString::from(format!("ef-context-{ci}")))
                    .h(px(row_h))
                    .px_2()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .hover(|s| s.bg(t.c("list.hoverBackground")))
                    .text_size(t.m("list.fontSize", 13.0))
                    .child(icon(if open { "chevronDown" } else { "chevronRight" }, px(14.), t.c("icon.foreground")))
                    .child(icon("database", t.m("explorer.iconSize", 15.0), t.c("icon.foreground")))
                    .child(div().whitespace_nowrap().font_weight(gpui::FontWeight::SEMIBOLD).child(c.info.name.clone()))
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_size(px(11.)).text_color(muted).child(project))
                    .on_click(cx.listener(move |wb, _, _, cx| wb.toggle_ef_context(key.clone(), cx))),
            );
            if !open {
                continue;
            }
            if let Some(note) = &c.note {
                list = list.child(div().pl(px(40.)).pr_3().py(px(2.)).text_size(px(11.)).text_color(t.c("errorForeground")).child(note.clone()));
            }
            if c.migrations.is_empty() {
                list = list.child(div().pl(px(40.)).h(px(row_h)).flex().items_center().text_size(px(12.)).text_color(muted).child("No migrations yet (Add Migration creates the first)."));
            }
            let count = c.migrations.len();
            for (mi, m) in c.migrations.iter().enumerate() {
                let applied = c.applied.as_ref().and_then(|a| a.get(&m.id).copied());
                let (mark, mark_color, label) = match applied {
                    Some(true) => ("check", passed, "applied"),
                    Some(false) => ("arrowUp", pending, "pending"),
                    None => ("file", t.c("icon.foreground"), ""),
                };
                let file = m.file.clone();
                let (project, context, id, last) = (c.info.project.to_string_lossy().into_owned(), c.info.name.clone(), m.id.clone(), mi + 1 == count);
                list = list.child(
                    div()
                        .id(SharedString::from(format!("ef-migration-{ci}-{mi}")))
                        .debug_selector(move || format!("ef-migration-{ci}-{mi}"))
                        .h(px(row_h))
                        .pl(px(40.))
                        .pr_3()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(t.m("explorer.itemSpacing", 6.0))
                        .cursor_pointer()
                        .hover(|s| s.bg(t.c("list.hoverBackground")))
                        .text_size(t.m("list.fontSize", 13.0))
                        .tooltip(tooltip(format!("{}\n{}", m.id, m.file.display())))
                        .child(icon(mark, px(14.), mark_color))
                        .child(div().whitespace_nowrap().child(m.name.clone()))
                        .child(div().flex_1())
                        .child(div().whitespace_nowrap().text_size(px(11.)).text_color(if applied == Some(false) { pending } else { muted }).child(label))
                        .on_click(cx.listener(move |wb, _, w, cx| wb.open_file(file.clone(), None, w, cx)))
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |wb, ev: &gpui::MouseDownEvent, _, cx| {
                                wb.ef_view.menu = Some(EfMenu { project: project.clone(), context: context.clone(), id: id.clone(), last, position: ev.position });
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        ),
                );
            }
        }

        let menu = state.menu.as_ref().map(|m| {
            let mut panel = crate::menu::panel(&t, 220.).on_mouse_down_out(cx.listener(|wb, _, _, cx| {
                wb.ef_view.menu = None;
                cx.notify();
            }));
            let entries: Vec<(&'static str, &'static str, &'static str)> = [
                Some(("ef-menu-update", "Update Database to Here", "update-to")),
                Some(("ef-menu-script", "Script from Here", "script")),
                m.last.then_some(("ef-menu-remove", "Remove", "remove")),
            ]
            .into_iter()
            .flatten()
            .collect();
            for (id, label, op) in entries {
                let (project, context) = (m.project.clone(), m.context.clone());
                // Remove takes no migration: it is always the last one.
                let arg = if op == "remove" { String::new() } else { m.id.clone() };
                panel = panel.child(
                    crate::menu::item(&t, id, label.to_owned(), None)
                        .debug_selector(move || id.to_owned())
                        .on_click(cx.listener(move |wb, _, w, cx| wb.run_ef_from_view(op, project.clone(), context.clone(), arg.clone(), w, cx))),
                );
            }
            gpui::deferred(gpui::anchored().position(m.position).snap_to_window().child(panel)).with_priority(1)
        });
        div().size_full().flex().flex_col().children(menu).child(top).child(list).into_any_element()
    }
}

#[cfg(test)]
mod view_tests {
    use crate::workbench::settings_page::view_tests::workbench;

    /// Contexts and migrations from the source; a click opens a migration's file; the right-click menu
    /// offers Remove on the last migration only.
    #[gpui::test]
    fn the_ef_view_lists_contexts_and_migrations(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "ef-view");
        let root = guard.dir.join("shop");
        let write = |rel: &str, text: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write("Shop/Shop.csproj", "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><TargetFramework>net10.0</TargetFramework></PropertyGroup></Project>");
        write("Shop/ShopContext.cs", "public class ShopContext : DbContext { }\n");
        for (id, name) in [("20260101000000_Init", "Init"), ("20260201000000_AddOrders", "AddOrders")] {
            write(
                &format!("Shop/Migrations/{id}.cs"),
                &format!("[DbContext(typeof(ShopContext))]\n[Migration(\"{id}\")]\npartial class {name} : Migration {{ }}\n"),
            );
        }
        wb.update_in(cx, |wb, window, cx| wb.open_folder(root.clone(), window, cx));
        cx.dispatch_action(crate::actions::ShowEfCore);
        for _ in 0..300 {
            cx.run_until_parked();
            if wb.update(cx, |wb, _| wb.ef_view.loaded) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let listed = wb.update(cx, |wb, _| {
            wb.ef_view.contexts.iter().map(|c| (c.info.name.clone(), c.migrations.iter().map(|m| m.name.clone()).collect::<Vec<_>>())).collect::<Vec<_>>()
        });
        assert_eq!(listed, [("ShopContext".to_owned(), vec!["Init".to_owned(), "AddOrders".to_owned()])]);

        // A click opens the migration's file.
        let bounds = cx.debug_bounds("ef-migration-0-0").expect("the migration row is drawn");
        cx.simulate_click(bounds.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        let open = wb.update(cx, |wb, _| wb.active.and_then(|i| wb.tabs.get(i)).and_then(|t| t.path.clone()));
        assert!(open.is_some_and(|p| p.ends_with("20260101000000_Init.cs")));

        // Right-click: Remove only on the last migration.
        let menu = |cx: &mut gpui::VisualTestContext, row: &'static str| {
            let bounds = cx.debug_bounds(row).unwrap();
            cx.simulate_mouse_down(bounds.center(), gpui::MouseButton::Right, gpui::Modifiers::none());
            cx.simulate_mouse_up(bounds.center(), gpui::MouseButton::Right, gpui::Modifiers::none());
            cx.run_until_parked();
            let shown = (cx.debug_bounds("ef-menu-update").is_some(), cx.debug_bounds("ef-menu-remove").is_some());
            wb.update(cx, |wb, cx| {
                wb.ef_view.menu = None;
                cx.notify();
            });
            cx.run_until_parked();
            shown
        };
        assert_eq!(menu(cx, "ef-migration-0-0"), (true, false));
        assert_eq!(menu(cx, "ef-migration-0-1"), (true, true));
        drop(guard);
    }
}
