//! Explorer events, moving files and the native file pickers.

use super::*;

impl Workbench {
    /// Renames or moves a file or folder on disk (never over an existing one) and retargets the open tabs
    /// of it or of anything below it.
    pub(super) fn move_path(&mut self, from: &Path, to: &Path, cx: &mut Context<Self>) -> anyhow::Result<()> {
        // A case-only rename on a case-insensitive file system reports the target as existing.
        let same_entry = from.parent() == to.parent() && from.to_string_lossy().to_lowercase() == to.to_string_lossy().to_lowercase();
        if to.exists() && !same_entry {
            anyhow::bail!("'{}' already exists", to.display());
        }
        std::fs::rename(from, to)?;
        for tab in &mut self.tabs {
            let Some(rest) = tab.path.as_deref().and_then(|p| p.strip_prefix(from).ok()) else { continue };
            let new = if rest.as_os_str().is_empty() { to.to_path_buf() } else { to.join(rest) };
            tab.title = new.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            tab.path = Some(new.clone());
            tab.editor.update(cx, |e, cx| e.set_path(new, cx));
        }
        self.explorer.update(cx, |e, cx| e.path_moved(from, to, cx));
        cx.notify();
        Ok(())
    }

    pub(super) fn on_explorer_event(&mut self, _: &Entity<Explorer>, event: &ExplorerEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            ExplorerEvent::Open(path, focus_editor) => {
                self.open_file(path.clone(), None, window, cx);
                if !focus_editor {
                    // Single click: keep the keyboard in the explorer (F2, …) like VS Code; double click goes to the editor.
                    let focus = self.explorer.focus_handle(cx);
                    window.focus(&focus);
                }
            }
            ExplorerEvent::Move { from, to_dir } => {
                let Some(name) = from.file_name() else { return };
                let target = to_dir.join(name);
                if target == *from {
                    return;
                }
                let result = if to_dir.starts_with(from) {
                    Err(anyhow::anyhow!("cannot move a folder into itself"))
                } else {
                    self.move_path(from, &target, cx)
                };
                if let Err(e) = result {
                    self.log(format!("Could not move {} to {}: {e}", from.display(), to_dir.display()));
                    self.show_panel(PanelView::Output, cx);
                }
            }
            ExplorerEvent::NewFile(dir) => self.open_prompt(PromptKind::NewFile(dir.clone()), "", window, cx),
            ExplorerEvent::NewFolder(dir) => self.open_prompt(PromptKind::NewFolder(dir.clone()), "", window, cx),
            ExplorerEvent::NewItem(dir) => self.open_template_picker(false, Some(dir.clone()), window, cx),
            ExplorerEvent::Rename(path) => {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.open_prompt(PromptKind::Rename(path.clone()), &name, window, cx);
            }
            ExplorerEvent::Delete(paths) => self.delete_to_trash(paths.clone(), window, cx),
            ExplorerEvent::DeletePermanently(paths) => self.delete_permanently(paths.clone(), None, window, cx),
            ExplorerEvent::Paste { from, to_dir } => {
                let (from, to_dir) = (from.clone(), to_dir.clone());
                let copy = cx.background_executor().spawn(async move {
                    let mut last = None;
                    let mut errors = Vec::new();
                    for src in &from {
                        // A folder pasted into itself or below it goes next to it instead (as "name copy").
                        let dir = if src.is_dir() && to_dir.starts_with(src) { src.parent().map(Path::to_path_buf) } else { Some(to_dir.clone()) };
                        let Some((dir, name)) = dir.zip(src.file_name()) else { continue };
                        let target = copy_target(&dir, &name.to_string_lossy());
                        match copy_recursive(src, &target) {
                            Ok(()) => last = Some(target),
                            Err(e) => errors.push(format!("Could not copy {} to {}: {e}", src.display(), dir.display())),
                        }
                    }
                    (last, errors)
                });
                cx.spawn_in(window, async move |this, cx| {
                    let (last, errors) = copy.await;
                    let _ = this.update(cx, |wb, cx| {
                        let failed = !errors.is_empty();
                        for e in errors {
                            wb.log(e);
                        }
                        if failed {
                            wb.show_panel(PanelView::Output, cx);
                        }
                        wb.explorer.update(cx, |e, cx| match &last {
                            Some(path) => e.reveal(path, cx),
                            None => e.refresh(cx),
                        });
                    });
                })
                .detach();
            }
            ExplorerEvent::OpenFolderRequested => window.dispatch_action(Box::new(OpenFolder), cx),
        }
    }

    /// Moves files or folders to the Recycle Bin / Trash after confirmation. When the system can't (a
    /// network drive, no trash on this volume), offers to delete them permanently instead.
    fn delete_to_trash(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let bin = if cfg!(windows) { "Recycle Bin" } else { "Trash" };
        let rx = window.prompt(PromptLevel::Info, &format!("Move {} to the {bin}?", describe(&paths)), None, &[&format!("Move to {bin}"), "Cancel"], cx);
        cx.spawn_in(window, async move |this, cx| {
            if !matches!(rx.await, Ok(0)) {
                return;
            }
            let targets = paths.clone();
            let result = move_to_trash(targets).await;
            let _ = this.update_in(cx, |wb, window, cx| match result {
                Ok(()) => wb.explorer.update(cx, |e, cx| e.refresh(cx)),
                Err(e) => {
                    // Some may be gone already; offer the rest.
                    let left: Vec<PathBuf> = paths.into_iter().filter(|p| p.exists()).collect();
                    wb.explorer.update(cx, |e, cx| e.refresh(cx));
                    if !left.is_empty() {
                        let detail = format!("It could not be moved to the {bin}: {e}");
                        wb.delete_permanently(left, Some(detail), window, cx);
                    }
                }
            });
        })
        .detach();
    }

    /// Deletes files or folders for good after confirmation; `detail` says why they aren't going to the trash.
    fn delete_permanently(&mut self, paths: Vec<PathBuf>, detail: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let rx = window.prompt(PromptLevel::Warning, &format!("Delete {} permanently?", describe(&paths)), detail.as_deref(), &["Delete Permanently", "Cancel"], cx);
        cx.spawn_in(window, async move |this, cx| {
            if !matches!(rx.await, Ok(0)) {
                return;
            }
            let errors = cx
                .background_executor()
                .spawn(async move {
                    paths
                        .iter()
                        .filter_map(|p| {
                            let result = if p.is_dir() { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) };
                            result.err().map(|e| format!("Could not delete {}: {e}", p.display()))
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |wb, cx| {
                let failed = !errors.is_empty();
                for e in errors {
                    wb.log(e);
                }
                if failed {
                    wb.show_panel(PanelView::Output, cx);
                }
                wb.explorer.update(cx, |e, cx| e.refresh(cx));
            });
        })
        .detach();
    }

    pub(super) fn pick_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Open Folder".into()) });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update_in(cx, |wb, window, cx| wb.open_folder(normalize_path(&path), window, cx));
            }
        })
        .detach();
    }

    pub(super) fn pick_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some("Open".into()) });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let _ = this.update_in(cx, |wb, window, cx| {
                    for path in paths {
                        wb.open_file(path, None, window, cx);
                    }
                });
            }
        })
        .detach();
    }

    // ───────────── git ─────────────

    /// Replace in the Search view: open files change in their editor (one undo step each, unsaved, so the
    /// change can be reviewed), the others on disk. Replace All asks first.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn replace_in_files(&mut self, files: Vec<PathBuf>, line: Option<usize>, matcher: bf_text::Matcher, template: String, count: usize, window: &mut Window, cx: &mut Context<Self>) {
        let files_n = files.len();
        let apply = move |wb: &mut Self, cx: &mut Context<Self>| {
            let (mut replaced, mut changed, mut failed) = (0usize, 0usize, Vec::new());
            for path in &files {
                let open = wb.tabs.iter().find(|t| t.is_document() && t.path.as_deref().is_some_and(|p| bf_core::paths::same_path(p, path))).map(|t| t.editor.clone());
                match open {
                    Some(editor) => {
                        let text = editor.read(cx).state.buffer.text();
                        let (new, n) = bf_core::search::replace_in_text(&text, &matcher, &template, line);
                        if n > 0 {
                            editor.update(cx, |e, cx| e.replace_text_minimal(&new, cx));
                            replaced += n;
                            changed += 1;
                        }
                    }
                    None => match bf_core::search::replace_in_file(path, &matcher, &template, line) {
                        Ok(0) => {}
                        Ok(n) => {
                            replaced += n;
                            changed += 1;
                        }
                        Err(e) => failed.push(format!("{}: {e}", path.display())),
                    },
                }
            }
            wb.log(format!("Replaced {replaced} match{} in {changed} file{}.", if replaced == 1 { "" } else { "es" }, if changed == 1 { "" } else { "s" }));
            for f in &failed {
                wb.log(format!("Could not replace in {f}"));
            }
            wb.reload_changed_tabs(cx);
            wb.search.update(cx, |s, cx| s.refresh(cx));
            cx.notify();
        };
        if line.is_some() {
            return apply(self, cx);
        }
        let question = format!("Replace {count} match{} in {files_n} file{}?", if count == 1 { "" } else { "es" }, if files_n == 1 { "" } else { "s" });
        let detail = "Files that are open change in their editor (unsaved, undo takes it back); the others are changed on disk.";
        let rx = window.prompt(PromptLevel::Warning, &question, Some(detail), &["Replace", "Cancel"], cx);
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(0) = rx.await {
                let _ = this.update(cx, |wb, cx| apply(wb, cx));
            }
        })
        .detach();
    }
}

/// Moves files or folders to the Recycle Bin / Trash on a thread of its own: on Windows `trash` sets up COM
/// for that thread (single-threaded, as the shell's file operations want), which panics on gpui's pool
/// threads because they are already set up multi-threaded.
async fn move_to_trash(paths: Vec<PathBuf>) -> Result<(), String> {
    let (tx, rx) = flume::bounded(1);
    let spawned = std::thread::Builder::new().name("bf-trash".into()).spawn(move || {
        let _ = tx.send(trash::delete_all(&paths).map_err(|e| e.to_string()));
    });
    if let Err(e) = spawned {
        return Err(e.to_string());
    }
    rx.recv_async().await.unwrap_or_else(|_| Err("the trash operation stopped unexpectedly".into()))
}

/// "'name'" for one item, "3 items" for more (for confirmations).
fn describe(paths: &[PathBuf]) -> String {
    match paths {
        [one] => format!("'{}'", one.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()),
        many => format!("{} items", many.len()),
    }
}

/// Where a copy of `name` goes in `dir`: `name` itself if free, else "stem copy.ext", "stem copy 2.ext", …
fn copy_target(dir: &Path, name: &str) -> PathBuf {
    let plain = dir.join(name);
    if !plain.exists() {
        return plain;
    }
    // ".env" has no extension, "a.tar.gz" keeps only ".gz" apart, like file managers do.
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => name.split_at(i),
        _ => (name, ""),
    };
    (1..)
        .map(|n| dir.join(if n == 1 { format!("{stem} copy{ext}") } else { format!("{stem} copy {n}{ext}") }))
        .find(|p| !p.exists())
        .expect("a free name")
}

/// Copies a file, or a folder with everything in it, to `to` (which must not exist yet).
fn copy_recursive(from: &Path, to: &Path) -> std::io::Result<()> {
    if !std::fs::metadata(from)?.is_dir() {
        return std::fs::copy(from, to).map(|_| ());
    }
    std::fs::create_dir(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_recursive(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod copy_tests {
    use super::{copy_recursive, copy_target};

    #[test]
    fn copies_get_free_names_and_folders_copy_deeply() {
        let dir = std::env::temp_dir().join("bf-copy-target");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src/nested")).unwrap();
        std::fs::write(dir.join("src/nested/a.cs"), "class A {}").unwrap();
        std::fs::write(dir.join("Program.cs"), "x").unwrap();
        std::fs::write(dir.join(".env"), "x").unwrap();

        assert_eq!(copy_target(&dir, "new.cs"), dir.join("new.cs"));
        assert_eq!(copy_target(&dir, "Program.cs"), dir.join("Program copy.cs"));
        assert_eq!(copy_target(&dir, ".env"), dir.join(".env copy"));
        assert_eq!(copy_target(&dir, "src"), dir.join("src copy"));

        copy_recursive(&dir.join("Program.cs"), &copy_target(&dir, "Program.cs")).unwrap();
        assert_eq!(copy_target(&dir, "Program.cs"), dir.join("Program copy 2.cs"));

        copy_recursive(&dir.join("src"), &copy_target(&dir, "src")).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("src copy/nested/a.cs")).unwrap(), "class A {}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod explorer_tests {
    use crate::actions::PressKeys;
    use super::*;
    use crate::workbench::settings_page::view_tests::workbench;

    /// The explorer works from the keyboard: arrows move and open/close folders, Enter opens a file, Ctrl+X
    /// then Ctrl+V moves, Ctrl+C then Ctrl+V copies next to the original.
    #[gpui::test]
    fn the_explorer_works_from_the_keyboard(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "explorer-keys");
        let ws = guard.dir.join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::create_dir_all(ws.join("docs")).unwrap();
        std::fs::write(ws.join("src/a.cs"), "class A {}").unwrap();
        std::fs::write(ws.join("src/b.cs"), "class B {}").unwrap();
        std::fs::write(ws.join("docs/x.md"), "# x").unwrap();
        std::fs::write(ws.join("README.md"), "readme").unwrap();
        // The long form (not "T86C6~1"), as the app opens folders and files.
        let ws = normalize_path(&ws);
        wb.update_in(cx, |wb, window, cx| {
            wb.open_folder(ws.clone(), window, cx);
            let focus = wb.explorer.focus_handle(cx);
            window.focus(&focus);
        });
        cx.run_until_parked();
        let selected = |cx: &mut gpui::VisualTestContext| {
            wb.update(cx, |wb, cx| wb.explorer.read(cx).selected_for_test().and_then(|p| p.strip_prefix(&ws).ok()).map(|p| p.to_string_lossy().replace('\\', "/")))
        };
        let press = |keys: &str, cx: &mut gpui::VisualTestContext| {
            cx.press(keys);
            cx.run_until_parked();
            selected(cx).unwrap_or_default()
        };

        // Rows: docs/, src/, README.md (folders first).
        assert_eq!(press("down", cx), "docs");
        assert_eq!(press("down", cx), "src");
        assert_eq!(press("right", cx), "src", "Right opens the folder");
        assert_eq!(press("right", cx), "src/a.cs", "Right on an open folder goes to its first child");
        assert_eq!(press("down", cx), "src/b.cs");
        assert_eq!(press("left", cx), "src", "Left on a file goes to its folder");
        assert_eq!(press("left", cx), "src", "Left closes the folder");
        assert_eq!(press("down", cx), "README.md", "the closed folder's files are skipped");
        assert_eq!(press("down", cx), "README.md", "Down stops at the end");
        assert_eq!(press("home", cx), "docs");
        assert_eq!(press("up", cx), "docs", "Up stops at the start");
        assert_eq!(press("end", cx), "README.md");

        press("enter", cx);
        let opened = wb.update(cx, |wb, _| wb.tabs.iter().any(|t| t.title == "README.md"));
        assert!(opened, "Enter opens the file");
        wb.update_in(cx, |wb, window, cx| {
            let focus = wb.explorer.focus_handle(cx);
            window.focus(&focus);
        });

        // Cut src/b.cs and paste it into docs/.
        press("up right right down", cx);
        assert_eq!(selected(cx).as_deref(), Some("src/b.cs"));
        press("ctrl-x home ctrl-v", cx);
        assert!(ws.join("docs/b.cs").exists() && !ws.join("src/b.cs").exists(), "Cut and Paste moves the file");

        // Copy README.md and paste it next to itself; a second paste gets the next free name.
        press("end ctrl-c ctrl-v", cx);
        press("end", cx);
        cx.press("ctrl-v");
        for _ in 0..50 {
            cx.run_until_parked();
            if ws.join("README copy 2.md").exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(std::fs::read_to_string(ws.join("README copy.md")).unwrap(), "readme");
        assert!(ws.join("README copy 2.md").exists());
        assert!(ws.join("README.md").exists(), "Copy keeps the original");
    }

    /// Shift+arrows and Ctrl+Click select several items; copy, cut and delete act on all of them, and Ctrl+A
    /// leaves out what's inside a selected folder (it goes along).
    #[gpui::test]
    fn the_explorer_selects_several_items(cx: &mut gpui::TestAppContext) {
        let (guard, wb, cx) = workbench(cx, "explorer-multi");
        let ws = guard.dir.join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::create_dir_all(ws.join("docs")).unwrap();
        std::fs::write(ws.join("src/a.cs"), "class A {}").unwrap();
        std::fs::write(ws.join("c.txt"), "c").unwrap();
        std::fs::write(ws.join("d.txt"), "d").unwrap();
        let ws = normalize_path(&ws);
        wb.update_in(cx, |wb, window, cx| {
            wb.open_folder(ws.clone(), window, cx);
            let focus = wb.explorer.focus_handle(cx);
            window.focus(&focus);
        });
        cx.run_until_parked();
        let selection = |cx: &mut gpui::VisualTestContext| {
            wb.update(cx, |wb, cx| {
                let names = wb.explorer.read(cx).selection_for_test();
                names.iter().map(|p| p.strip_prefix(&ws).unwrap().to_string_lossy().replace('\\', "/")).collect::<Vec<_>>()
            })
        };
        let press = |keys: &str, cx: &mut gpui::VisualTestContext| {
            cx.press(keys);
            cx.run_until_parked();
        };
        let wait_until = |cx: &mut gpui::VisualTestContext, done: &dyn Fn() -> bool| {
            for _ in 0..100 {
                cx.run_until_parked();
                if done() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        };

        // Rows: docs/, src/, c.txt, d.txt. Shift+Up from the last row selects both files.
        press("end shift-up", cx);
        assert_eq!(selection(cx), ["c.txt", "d.txt"]);
        // Copy both into src/.
        press("ctrl-c home down ctrl-v", cx);
        wait_until(cx, &|| ws.join("src/d.txt").exists());
        assert!(ws.join("src/c.txt").exists() && ws.join("src/d.txt").exists(), "both files are copied");

        // Ctrl+Click picks c.txt and d.txt; cut them and paste into docs/.
        let ctrl = gpui::Modifiers::secondary_key();
        let click = |name: &str, modifiers: gpui::Modifiers, cx: &mut gpui::VisualTestContext| {
            let path = ws.join(name);
            wb.update(cx, |wb, cx| wb.explorer.update(cx, |e, cx| e.click_for_test(&path, modifiers, cx)));
            cx.run_until_parked();
        };
        click("c.txt", gpui::Modifiers::none(), cx);
        click("d.txt", ctrl, cx);
        assert_eq!(selection(cx), ["c.txt", "d.txt"]);
        // Ctrl+Click again takes one out.
        click("c.txt", ctrl, cx);
        assert_eq!(selection(cx), ["d.txt"]);
        click("c.txt", ctrl, cx);
        press("ctrl-x home ctrl-v", cx);
        assert!(ws.join("docs/c.txt").exists() && ws.join("docs/d.txt").exists(), "both files are moved");
        assert!(!ws.join("c.txt").exists() && !ws.join("d.txt").exists());

        // src/ is open since the paste revealed a file in it; Right goes to its first file. Select everything:
        // the files in src/ go along with it.
        press("home down right ctrl-a", cx);
        assert_eq!(selection(cx), ["docs", "src"]);
        press("escape", cx);
        assert_eq!(selection(cx), ["src/a.cs"], "Escape keeps only the keyboard row");

        // Delete several at once (permanently, to keep the test machine's trash clean).
        press("ctrl-a shift-delete", cx);
        cx.simulate_prompt_answer("Delete Permanently");
        wait_until(cx, &|| !ws.join("src").exists() && !ws.join("docs").exists());
        assert!(!ws.join("src").exists() && !ws.join("docs").exists(), "both folders are deleted");
    }
}
