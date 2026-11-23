//! `.resx` files open in the resource editor (`crate::resx_view`): one tab per resource with all its
//! languages. Problems lists the keys a language is missing, for every resource in the workspace.

use crate::resx_view::{ResxEvent, ResxView};

use super::*;

impl Workbench {
    /// Opens the resource `path` belongs to in the resource editor (or the tab that has it already).
    pub(super) fn open_resx(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.tabs.iter().position(|t| matches!(&t.page, Some(Page::Resx(v)) if v.read(cx).contains(path))) {
            return self.activate(ix, window, cx);
        }
        let view = cx.new(|cx| ResxView::new(path, window, cx));
        let editor = cx.new(|cx| {
            let mut e = EditorView::new("", None, cx);
            e.read_only = true;
            e
        });
        let title = view.read(cx).title();
        self.add_tab(editor, None, title, false, window, cx);
        let sub = cx.subscribe_in(&view, window, Self::on_resx_event);
        if let Some(tab) = self.tabs.last_mut() {
            tab.page = Some(Page::Resx(view));
            tab._subs.push(sub);
        }
        cx.notify();
    }

    fn on_resx_event(&mut self, _: &Entity<ResxView>, event: &ResxEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            ResxEvent::Changed => cx.notify(),
            ResxEvent::Saved => {
                self.refresh_resx_problems(cx);
                self.scm.update(cx, |s, cx| s.refresh(cx));
            }
            ResxEvent::OpenFile(path) => self.open_file_text(path.clone(), None, window, cx),
        }
    }

    /// Ctrl+S on a resource editor tab: writes the files that changed.
    pub(super) fn save_resx(&mut self, view: Entity<ResxView>, window: &mut Window, cx: &mut Context<Self>) {
        match view.update(cx, |v, cx| v.save(window, cx)) {
            Ok(written) => {
                for path in written {
                    for p in &self.plugins {
                        p.event("fileSaved", Some(&path));
                    }
                }
            }
            Err(e) => {
                self.log(format!("{e:#}"));
                self.show_panel(PanelView::Output, cx);
            }
        }
    }

    /// Files changed on disk: resource editors without edits read their files again.
    pub(super) fn reload_resx_views(&mut self, cx: &mut Context<Self>) {
        let views: Vec<Entity<ResxView>> = self.tabs.iter().filter_map(|t| match &t.page {
            Some(Page::Resx(v)) => Some(v.clone()),
            _ => None,
        }).collect();
        for v in views {
            v.update(cx, |v, cx| v.reload_if_unchanged(cx));
        }
    }

    /// Looks for keys missing in a language in every resource of the workspace (off the UI thread).
    pub(super) fn refresh_resx_problems(&mut self, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        self.resx_check = Some(cx.spawn(async move |this, cx| {
            let problems = cx.background_executor().spawn(async move { resx_problems(&workspace.files()) }).await;
            let _ = this.update(cx, |wb, cx| {
                wb.resx_check = None;
                if wb.resx_problems.len() != problems.len() || !problems.is_empty() || !wb.resx_problems.is_empty() {
                    wb.resx_problems = problems;
                    cx.notify();
                }
            });
        }));
    }
}

/// A warning on the neutral file's key for each language that lacks it.
fn resx_problems(files: &[PathBuf]) -> Vec<(PathBuf, crate::editor::Diagnostic)> {
    let mut out = Vec::new();
    for family in bf_core::resx::families(files) {
        let Ok(neutral) = std::fs::read_to_string(&family.neutral) else { continue };
        let Ok(neutral) = bf_core::resx::parse(&neutral) else { continue };
        let cultures: Vec<(String, Vec<bf_core::resx::Entry>)> = family
            .cultures
            .iter()
            .filter_map(|(c, p)| Some((c.clone(), bf_core::resx::parse(&std::fs::read_to_string(p).ok()?).ok()?)))
            .collect();
        for m in bf_core::resx::missing(&neutral, &cultures) {
            let file = format!("{}.{}.resx", family.name, m.culture);
            out.push((
                family.neutral.clone(),
                crate::editor::Diagnostic {
                    start: Point::new(m.line, 0),
                    end: Point::new(m.line, 1),
                    severity: bf_lsp::Severity::Warning,
                    message: format!("{} has no {} translation ({file})", m.key, m.culture),
                    code: Some("resx".into()),
                },
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use crate::actions::PressKeys;
    use crate::resx_view::tests::{GERMAN, NEUTRAL};
    use crate::workbench::settings_page::view_tests::workbench;

    /// Opening a .resx opens the resource editor with every language; a missing translation is in
    /// Problems until it's typed into its cell and saved with Ctrl+S, which writes only that entry.
    #[gpui::test]
    fn resx_files_open_as_a_table_and_save(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "resx");
        let root = guard.dir.join("app");
        std::fs::create_dir_all(root.join("Resources")).unwrap();
        std::fs::write(root.join("Resources/Strings.resx"), NEUTRAL).unwrap();
        std::fs::write(root.join("Resources/Strings.de.resx"), GERMAN).unwrap();
        wb.update_in(cx, |wb, window, cx| {
            wb.open_folder(root.clone(), window, cx);
            wb.refresh_resx_problems(cx);
        });
        let settle = |cx: &mut gpui::VisualTestContext| {
            for _ in 0..200 {
                cx.run_until_parked();
                if wb.update(cx, |wb, _| wb.resx_check.is_none()) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        settle(cx);
        let problems = wb.update(cx, |wb, cx| wb.problems(cx));
        let messages: Vec<&str> = problems.iter().map(|(_, d)| d.message.as_str()).collect();
        assert_eq!(messages, ["Bye has no de translation (Strings.de.resx)"]);

        wb.update_in(cx, |wb, window, cx| wb.open_file(root.join("Resources/Strings.de.resx"), None, window, cx));
        cx.run_until_parked();
        let title = wb.update(cx, |wb, _| wb.tabs[wb.active.unwrap()].title.clone());
        assert_eq!(title, "Strings.resx");
        // The other language's file goes to the same tab.
        wb.update_in(cx, |wb, window, cx| wb.open_file(root.join("Resources/Strings.resx"), None, window, cx));
        assert_eq!(wb.update(cx, |wb, _| wb.tabs.len()), 1);

        let cell = cx.debug_bounds("resx-cell-Bye-de").expect("the missing cell is drawn");
        cx.simulate_click(cell.center(), gpui::Modifiers::none());
        cx.simulate_input("Tschüss");
        cx.press("enter");
        cx.run_until_parked();
        assert!(wb.update(cx, |wb, cx| wb.tabs[0].modified(cx)), "the tab shows unsaved edits");
        cx.press("ctrl-s");
        settle(cx);
        assert!(!wb.update(cx, |wb, cx| wb.tabs[0].modified(cx)));
        let de = std::fs::read_to_string(root.join("Resources/Strings.de.resx")).unwrap();
        assert_eq!(de, GERMAN.replace("  </data>\n</root>", "  </data>\n  <data name=\"Bye\" xml:space=\"preserve\">\n    <value>Tschüss</value>\n  </data>\n</root>"));
        assert_eq!(std::fs::read_to_string(root.join("Resources/Strings.resx")).unwrap(), NEUTRAL, "the unchanged file isn't written");
        assert!(wb.update(cx, |wb, cx| wb.problems(cx)).is_empty(), "nothing missing any more");
        drop(guard);
    }
}
