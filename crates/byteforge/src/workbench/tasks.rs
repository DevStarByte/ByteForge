//! The Tasks view and "Tasks: Run Task…": the workspace's tasks (`.byteforge/tasks.json`, package.json
//! scripts, Makefile targets, justfile recipes; see `bf_core::tasks`), each run in a terminal of its own.

use super::*;
use std::collections::HashSet;

use bf_core::tasks::{self as workspace_tasks, Source, Task as WorkspaceTask, Tasks};

#[derive(Default)]
pub(super) struct TasksState {
    list: Tasks,
    loaded: bool,
    loading: Option<Task<()>>,
    /// The task "Rerun Last Task" runs.
    last: Option<WorkspaceTask>,
    /// Groups the user closed in the view.
    collapsed: HashSet<String>,
}

type AfterRefresh = Box<dyn FnOnce(&mut Workbench, &mut Context<Workbench>)>;

impl Workbench {
    /// Reads the workspace's tasks again, off the UI thread; `then` runs once they are there.
    pub(super) fn refresh_tasks(&mut self, then: Option<AfterRefresh>, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.root() else {
            self.tasks.list = Tasks::default();
            self.tasks.loaded = true;
            if let Some(then) = then {
                then(self, cx);
            }
            return;
        };
        self.tasks.loading = Some(cx.spawn(async move |this, cx| {
            let found = cx.background_executor().spawn(async move { workspace_tasks::discover(&root) }).await;
            let _ = this.update(cx, |wb, cx| {
                wb.tasks.list = found;
                wb.tasks.loaded = true;
                wb.tasks.loading = None;
                if let Some(then) = then {
                    then(wb, cx);
                }
                cx.notify();
            });
        }));
    }

    /// Runs `task` in a new terminal named after it (saving all files first unless it says not to).
    pub(super) fn run_task(&mut self, task: WorkspaceTask, window: &mut Window, cx: &mut Context<Self>) {
        if task.save_all {
            self.save_all(window, cx);
        }
        self.run_in_terminal(task.name.clone(), task.command.clone(), Some(task.cwd.clone()), window, cx);
        self.tasks.last = Some(task);
    }

    pub(super) fn run_task_by_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        match self.tasks.list.tasks.iter().find(|t| t.key() == key).cloned() {
            Some(task) => self.run_task(task, window, cx),
            None => {
                self.log("That task isn't there any more (Tasks: Refresh Tasks reads them again).");
                self.show_panel(PanelView::Output, cx);
            }
        }
    }

    /// "Tasks: Run Task…": a pick list of every task, the last one run first.
    pub(super) fn open_task_picker(&mut self, cx: &mut Context<Self>) {
        self.refresh_tasks(
            // The picker needs the window, which a background refresh's callback doesn't have.
            Some(Box::new(|_, cx| {
                Self::with_window(cx, |wb, window, cx| {
                    if let Some(error) = wb.tasks.list.error.clone() {
                        wb.log(format!("Tasks: {error}"));
                    }
                    let last = wb.tasks.last.as_ref().map(WorkspaceTask::key);
                    let mut items: Vec<PaletteItem> = wb
                        .tasks
                        .list
                        .tasks
                        .iter()
                        .map(|t| {
                            let mut item = PaletteItem::new(t.name.clone(), PaletteAction::Command(Box::new(RunTaskByKey { key: t.key() })));
                            item.detail = Some(t.command.clone());
                            item.hint = Some(if last.as_deref() == Some(t.key().as_str()) { "last run".to_owned() } else { t.group.clone() });
                            item.search = Some(format!("{} {}", t.group, t.name));
                            item
                        })
                        .collect();
                    if let Some(i) = last.and_then(|key| wb.tasks.list.tasks.iter().position(|t| t.key() == key)) {
                        let item = items.remove(i);
                        items.insert(0, item);
                    }
                    let mut configure = PaletteItem::new("Configure Tasks", PaletteAction::Command(Box::new(ConfigureTasks)));
                    configure.detail = Some(format!("Edit {}/{}", workspace_tasks::FOLDER, workspace_tasks::FILE));
                    items.push(configure);
                    let placeholder = if items.len() == 1 { "No tasks in this folder yet" } else { "Select the task to run" };
                    wb.open_pick_palette(items, placeholder, window, cx);
                });
            })),
            cx,
        );
    }

    /// "Tasks: Rerun Last Task" (the picker when nothing has run yet).
    pub(super) fn rerun_last_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.tasks.last.clone() {
            Some(task) => self.run_task(task, window, cx),
            None => self.open_task_picker(cx),
        }
    }

    /// "Tasks: Configure Tasks": opens `.byteforge/tasks.json`, created with an example when it isn't there.
    pub(super) fn configure_tasks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.root() else {
            self.log("Open a folder first: its tasks live in .byteforge/tasks.json.");
            self.show_panel(PanelView::Output, cx);
            return;
        };
        let file = workspace_tasks::tasks_file(&root);
        if !file.exists() {
            let written = file.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::write(&file, workspace_tasks::EXAMPLE));
            if let Err(e) = written {
                self.log(format!("Could not create {}: {e}", file.display()));
                self.show_panel(PanelView::Output, cx);
                return;
            }
            self.refresh_tasks(None, cx);
        }
        self.open_file(file, None, window, cx);
    }

    fn toggle_task_group(&mut self, group: String, cx: &mut Context<Self>) {
        if !self.tasks.collapsed.remove(&group) {
            self.tasks.collapsed.insert(group);
        }
        cx.notify();
    }

    /// The Tasks side view: the tasks under their group's header; a click runs one.
    pub(super) fn render_tasks_view(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = Theme::global(cx);
        let muted = t.c("foreground.muted");
        let row_h = t.n("list.itemHeight", 22.0);
        if self.workspace.root().is_none() {
            return div().p_3().text_size(px(12.)).text_color(muted).child("Open a folder to see its tasks.").into_any_element();
        }
        let small = |id: &'static str, name: &str, tip: &str| {
            div()
                .id(id)
                .p(px(2.))
                .rounded(px(3.))
                .cursor_pointer()
                .hover(|s| s.bg(t.c("toolbar.hoverBackground")))
                .tooltip(tooltip(tip.to_owned()))
                .child(icon(name, px(14.), t.c("icon.foreground")))
        };
        let list = &self.tasks.list;
        let status = match (&list.error, self.tasks.loaded) {
            (Some(e), _) => div().text_color(t.c("errorForeground")).child(e.clone()),
            (None, false) => div().child("Looking for tasks…"),
            (None, true) => div().child(match list.tasks.len() {
                1 => "1 task".to_owned(),
                n => format!("{n} tasks"),
            }),
        };
        let top = div()
            .px_3()
            .flex()
            .items_center()
            .gap_1()
            .min_h(px(22.))
            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_size(px(12.)).text_color(muted).child(status))
            .child(small("tasks-run", "play", "Run Task…").on_click(cx.listener(|wb, _, _, cx| wb.open_task_picker(cx))))
            .child(small("tasks-refresh", "refresh", "Refresh Tasks").on_click(cx.listener(|wb, _, _, cx| wb.refresh_tasks(None, cx))))
            .child(small("tasks-configure", "settings", "Configure Tasks (.byteforge/tasks.json)").on_click(cx.listener(|wb, _, w, cx| wb.configure_tasks(w, cx))));

        let mut body = div().id("tasks-list").flex_1().min_h_0().overflow_y_scroll().flex().flex_col();
        if self.tasks.loaded && list.tasks.is_empty() {
            body = body.child(
                div()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .text_size(px(12.))
                    .text_color(muted)
                    .child("No tasks found. Tasks come from .byteforge/tasks.json, package.json scripts, Makefiles and justfiles.")
                    .child(
                        div()
                            .id("tasks-configure-empty")
                            .px_2()
                            .py_1()
                            .rounded(px(3.))
                            .cursor_pointer()
                            .bg(t.bg("button.background"))
                            .text_color(t.c("button.foreground"))
                            .flex()
                            .justify_center()
                            .child("Configure Tasks")
                            .on_click(cx.listener(|wb, _, w, cx| wb.configure_tasks(w, cx))),
                    ),
            );
        }
        let mut groups: Vec<&str> = Vec::new();
        for task in &list.tasks {
            if !groups.contains(&task.group.as_str()) {
                groups.push(&task.group);
            }
        }
        for (gi, group) in groups.into_iter().enumerate() {
            let open = !self.tasks.collapsed.contains(group);
            let name = group.to_owned();
            body = body.child(
                div()
                    .id(SharedString::from(format!("task-group-{gi}")))
                    .h(px(row_h))
                    .px_2()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .hover(|s| s.bg(t.c("list.hoverBackground")))
                    .text_size(t.m("sideBarSectionHeader.fontSize", 11.0))
                    .font_weight(t.weight("sideBarSectionHeader.weight", gpui::FontWeight::BOLD))
                    .text_color(t.c("sideBarSectionHeader.foreground"))
                    .child(icon(if open { "chevronDown" } else { "chevronRight" }, px(14.), t.c("icon.foreground")))
                    .child(div().whitespace_nowrap().child(name.clone()))
                    .on_click(cx.listener(move |wb, _, _, cx| wb.toggle_task_group(name.clone(), cx))),
            );
            if !open {
                continue;
            }
            for (ti, task) in list.tasks.iter().enumerate().filter(|(_, task)| task.group == group) {
                let key = task.key();
                let glyph = match task.source {
                    Source::Workspace => "tasks",
                    Source::Npm => "fileScript",
                    Source::Make | Source::Just => "build",
                };
                body = body.child(
                    div()
                        .id(SharedString::from(format!("task-row-{ti}")))
                        .debug_selector(move || format!("task-row-{ti}"))
                        .group("task-row")
                        .h(px(row_h))
                        .pl(px(24.))
                        .pr_3()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(t.m("explorer.itemSpacing", 6.0))
                        .cursor_pointer()
                        .hover(|s| s.bg(t.c("list.hoverBackground")))
                        .text_size(t.m("list.fontSize", 13.0))
                        .tooltip(tooltip(format!("{}\n{}", task.command, task.cwd.display())))
                        .child(icon(glyph, t.m("explorer.iconSize", 15.0), t.c("icon.foreground")))
                        .child(div().whitespace_nowrap().child(task.name.clone()))
                        .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_size(px(11.)).text_color(muted).child(task.command.clone()))
                        .child(div().invisible().group_hover("task-row", |s| s.visible()).child(icon("play", px(14.), t.c("icon.foreground"))))
                        .on_click(cx.listener(move |wb, _, w, cx| wb.run_task_by_key(&key, w, cx))),
                );
            }
        }
        div().size_full().flex().flex_col().child(top).child(body).into_any_element()
    }
}

#[cfg(test)]
mod view_tests {
    use crate::workbench::settings_page::view_tests::workbench;

    /// The view lists the tasks file's tasks and a Makefile's targets; a click runs one in a terminal named
    /// after it; Run Task… and Rerun Last Task; Configure Tasks writes the example file.
    #[gpui::test]
    fn the_tasks_view_lists_and_runs_tasks(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "tasks-view");
        let root = guard.dir.join("ws");
        std::fs::create_dir_all(root.join(".byteforge")).unwrap();
        std::fs::write(root.join(".byteforge/tasks.json"), "{ \"tasks\": [ { \"name\": \"Hello\", \"command\": \"echo hello-from-task\" } ] }").unwrap();
        std::fs::write(root.join("Makefile"), "all:\n\techo all\n").unwrap();
        wb.update_in(cx, |wb, window, cx| wb.open_folder(root.clone(), window, cx));
        cx.dispatch_action(crate::actions::ShowTasks);
        for _ in 0..200 {
            cx.run_until_parked();
            if wb.update(cx, |wb, _| wb.tasks.loaded) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let names = wb.update(cx, |wb, _| wb.tasks.list.tasks.iter().map(|t| (t.group.clone(), t.name.clone())).collect::<Vec<_>>());
        assert_eq!(names, [("Tasks".to_owned(), "Hello".to_owned()), ("Makefile".to_owned(), "all".to_owned())]);

        // A click on the row runs it: a new terminal named after the task.
        let terminals = |wb: &gpui::Entity<super::Workbench>, cx: &mut gpui::VisualTestContext| wb.update(cx, |wb, cx| wb.terminals.iter().map(|(t, _)| t.read(cx).label.clone().unwrap_or_default()).collect::<Vec<_>>());
        let before = terminals(&wb, cx).len();
        let bounds = cx.debug_bounds("task-row-0").expect("the task row is drawn");
        cx.simulate_click(bounds.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        let after = terminals(&wb, cx);
        assert_eq!(after.len(), before + 1);
        assert_eq!(after.last().map(String::as_str), Some("Hello"));

        // Rerun Last Task runs it again.
        cx.dispatch_action(crate::actions::RerunLastTask);
        cx.run_until_parked();
        assert_eq!(terminals(&wb, cx).len(), before + 2);

        // Configure Tasks opens the existing file (and doesn't overwrite it).
        cx.dispatch_action(crate::actions::ConfigureTasks);
        cx.run_until_parked();
        let open = wb.update(cx, |wb, _| wb.tabs.iter().any(|t| t.path.as_deref().is_some_and(|p| p.ends_with(".byteforge/tasks.json"))));
        assert!(open);
        assert!(std::fs::read_to_string(root.join(".byteforge/tasks.json")).unwrap().contains("hello-from-task"));
        drop(guard);
    }

    #[gpui::test]
    fn configure_tasks_writes_the_example(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "tasks-configure");
        let root = guard.dir.join("empty");
        std::fs::create_dir_all(&root).unwrap();
        wb.update_in(cx, |wb, window, cx| wb.open_folder(root.clone(), window, cx));
        cx.dispatch_action(crate::actions::ConfigureTasks);
        cx.run_until_parked();
        let text = std::fs::read_to_string(root.join(".byteforge/tasks.json")).unwrap();
        assert_eq!(text, bf_core::tasks::EXAMPLE);
        drop(guard);
    }
}
