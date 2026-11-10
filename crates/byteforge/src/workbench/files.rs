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
            ExplorerEvent::Rename(path) => {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.open_prompt(PromptKind::Rename(path.clone()), &name, window, cx);
            }
            ExplorerEvent::Delete(path) => {
                let path = path.clone();
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let rx = window.prompt(PromptLevel::Warning, &format!("Delete '{name}' permanently?"), None, &["Delete", "Cancel"], cx);
                cx.spawn_in(window, async move |this, cx| {
                    if let Ok(0) = rx.await {
                        let result = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
                        let _ = this.update(cx, |wb, cx| {
                            if let Err(e) = result {
                                wb.log(format!("Could not delete {}: {e}", path.display()));
                            }
                            wb.explorer.update(cx, |e, cx| e.refresh(cx));
                        });
                    }
                })
                .detach();
            }
            ExplorerEvent::OpenFolderRequested => window.dispatch_action(Box::new(OpenFolder), cx),
        }
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
}
