//! The command palette and text prompts.

use super::*;

impl Workbench {
    pub(super) fn open_palette(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_open = None;
        let (items, placeholder, initial) = self.palette_items(mode, window, cx);
        let palette = cx.new(|cx| Palette::new(mode, items, placeholder, initial, window, cx));
        if mode == Mode::Themes {
            self.palette_restore_theme = Some(Theme::global(cx).data.name.clone());
            palette.update(cx, |p, cx| p.select_hint("current", cx));
        }
        let sub = cx.subscribe_in(&palette, window, Self::on_palette_event);
        palette.update(cx, |p, cx| p.focus_input(window, cx));
        self.palette = Some((palette, sub));
        self.load_palette_symbols(mode, String::new(), cx);
        cx.notify();
    }

    pub(super) fn palette_items(&self, mode: Mode, window: &Window, cx: &App) -> (Vec<PaletteItem>, &'static str, &'static str) {
        match mode {
            Mode::Commands => {
                let mut items: Vec<PaletteItem> = cx
                    .global::<Commands>()
                    .list
                    .iter()
                    .map(|c| {
                        let mut item = PaletteItem::new(c.display_title(), PaletteAction::Command(c.action.boxed_clone()));
                        let key = window.keystroke_text_for(c.action.as_ref());
                        if !key.is_empty() && key != format!("{:?}", c.action.name()) && !key.contains("::") {
                            item.hint = Some(pretty_keys(&key));
                        }
                        item
                    })
                    .collect();
                items.sort_by_key(|a| a.label.to_lowercase());
                (items, "Type a command", ">")
            }
            Mode::Files => {
                let root = self.workspace.root();
                let items = self
                    .workspace
                    .files()
                    .iter()
                    .map(|path| {
                        let rel = root.as_ref().and_then(|r| path.strip_prefix(r).ok()).unwrap_or(path);
                        let mut item = PaletteItem::new(path.file_name().unwrap_or_default().to_string_lossy(), PaletteAction::OpenFile(path.clone()));
                        item.detail = rel.parent().map(|p| p.display().to_string()).filter(|s| !s.is_empty());
                        item.search = Some(rel.display().to_string());
                        item
                    })
                    .collect();
                (items, "Search files by name (type > for commands, : for line)", "")
            }
            Mode::Themes => {
                let current = Theme::global(cx).data.name.clone();
                let items = self
                    .themes
                    .available()
                    .into_iter()
                    .map(|t| {
                        let mut item = PaletteItem::new(t.name.clone(), PaletteAction::Theme(t.name.clone()));
                        item.detail = t.description.clone().or_else(|| t.user.then(|| "user theme".into()));
                        item.hint = Some(if t.name.eq_ignore_ascii_case(&current) { "current".into() } else { t.kind.clone() });
                        item.swatches = t.swatches.clone();
                        item
                    })
                    .collect();
                (items, "Select a color theme (↑↓ to preview)", "")
            }
            Mode::GoToLine => (Vec::new(), "Type a line number", ":"),
            Mode::Symbols => (Vec::new(), "Go to a symbol in this file", "@"),
            Mode::WorkspaceSymbols => (Vec::new(), "Type the name of a type or member", "#"),
            Mode::Prompt | Mode::Branches | Mode::Pick | Mode::Packages => (Vec::new(), "Name", ""),
        }
    }

    pub(super) fn open_prompt(&mut self, kind: PromptKind, initial: &str, window: &mut Window, cx: &mut Context<Self>) {
        let template_title;
        let placeholder = match &kind {
            PromptKind::NewFile(_) => "New file name",
            PromptKind::NewFolder(_) => "New folder name",
            PromptKind::Rename(_) => "New name",
            PromptKind::CustomizeTheme => "Name for your theme (inherits everything from the current theme)",
            PromptKind::CreateBranch => "New branch name (created from the current HEAD and checked out)",
            PromptKind::Template(t) if t.project => "Project name (a folder with this name is created)",
            PromptKind::Template(t) => {
                template_title = format!("{} name", t.title);
                template_title.as_str()
            }
            PromptKind::RenameSymbol { .. } => "New name (Enter to rename everywhere, Escape to cancel)",
            PromptKind::EfMigrationName(_) => "Migration name, e.g. AddOrders (EF Core: dotnet ef migrations add)",
        };
        self.prompt = Some(kind);
        let palette = cx.new(|cx| Palette::new(Mode::Prompt, Vec::new(), placeholder, initial, window, cx));
        let sub = cx.subscribe_in(&palette, window, Self::on_palette_event);
        palette.update(cx, |p, cx| p.focus_input(window, cx));
        self.palette = Some((palette, sub));
        cx.notify();
    }

    pub(super) fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette = None;
        self.prompt = None;
        if let Some(editor) = self.active_editor() {
            editor.update(cx, |e, _| e.focus(window));
        } else {
            window.focus(&self.focus);
        }
        cx.notify();
    }

    pub(super) fn on_palette_event(&mut self, palette: &Entity<Palette>, event: &PaletteEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            PaletteEvent::SwitchMode(mode) => {
                let (items, _, _) = self.palette_items(*mode, window, cx);
                palette.update(cx, |p, cx| p.set_items(*mode, items, cx));
                self.load_palette_symbols(*mode, String::new(), cx);
            }
            PaletteEvent::Query(query) if palette.read(cx).mode == Mode::Packages => self.search_packages(query.clone(), cx),
            PaletteEvent::Query(query) => self.load_palette_symbols(Mode::WorkspaceSymbols, query.clone(), cx),
            PaletteEvent::Preview(PaletteAction::Theme(name)) => {
                let name = name.clone();
                self.apply_theme(&name, window, cx);
            }
            PaletteEvent::Preview(_) => {}
            PaletteEvent::Dismissed => {
                if let Some(name) = self.palette_restore_theme.take() {
                    self.apply_theme(&name, window, cx);
                }
                self.close_palette(window, cx);
            }
            PaletteEvent::Accept(action) => {
                let action = action.clone();
                self.palette_restore_theme = None;
                let prompt = self.prompt.take();
                self.close_palette(window, cx);
                match action {
                    PaletteAction::Command(a) => window.dispatch_action(a, cx),
                    PaletteAction::OpenFile(path) => self.open_file(path, None, window, cx),
                    PaletteAction::Theme(name) => {
                        self.apply_theme(&name, window, cx);
                        self.settings.set("workbench.theme", json!(name));
                    }
                    PaletteAction::GoToLine(line) => {
                        if let Some(editor) = self.active_editor() {
                            editor.update(cx, |e, cx| e.go_to(Point::new(line, 0), cx));
                        }
                    }
                    PaletteAction::Branch(branch) => self.scm.update(cx, |s, cx| s.checkout(branch, cx)),
                    PaletteAction::GoTo(path, at) => self.open_file(path, Some(at), window, cx),
                    PaletteAction::Package(id, _latest) => {
                        if let Some(project) = self.package_target.take() {
                            self.open_version_picker(project, id, window, cx);
                        }
                    }
                    PaletteAction::Prompt(text) => {
                        if let Some(kind) = prompt {
                            self.complete_prompt(kind, text, window, cx);
                        }
                    }
                }
            }
        }
    }

    pub(super) fn complete_prompt(&mut self, kind: PromptKind, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let text = text.trim().to_owned();
        if text.is_empty() {
            return;
        }
        let result: anyhow::Result<()> = (|| {
            match kind {
                PromptKind::NewFile(dir) => {
                    let path = dir.join(&text);
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    if !path.exists() {
                        std::fs::write(&path, "")?;
                    }
                    self.explorer.update(cx, |e, cx| e.refresh(cx));
                    self.open_file(path, None, window, cx);
                }
                PromptKind::NewFolder(dir) => {
                    std::fs::create_dir_all(dir.join(&text))?;
                    self.explorer.update(cx, |e, cx| e.refresh(cx));
                }
                PromptKind::Rename(path) => {
                    let target = path.parent().map(|p| p.join(&text)).unwrap_or_else(|| PathBuf::from(&text));
                    if target != path {
                        self.move_path(&path, &target, cx)?;
                    }
                }
                PromptKind::RenameSymbol { editor, at, old } => {
                    if text != old
                        && let Some(editor) = editor.upgrade()
                    {
                        editor.update(cx, |e, cx| e.rename_symbol(at, text.clone(), cx));
                    }
                }
                PromptKind::Template(template) => self.create_from_template(template, text.clone(), window, cx)?,
                PromptKind::EfMigrationName(cmd) => self.ef_migration_named(cmd, &text, window, cx),
                PromptKind::CreateBranch => {
                    let name = text.replace(' ', "-");
                    self.scm.update(cx, |s, cx| s.create_branch(name, cx));
                }
                PromptKind::CustomizeTheme => {
                    let current = Theme::global(cx).data.clone();
                    let file = text.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect::<String>() + ".json";
                    let path = self.paths.user_themes().join(file);
                    if !path.exists() {
                        let body = format!(
                            "{{\n  // Saved changes apply instantly. Every key of the inherited theme can be overridden here.\n  // See themes/byteforge-dark.json next to ByteForge for the full list of keys.\n  \"name\": \"{text}\",\n  \"type\": \"{}\",\n  \"inherits\": \"{}\",\n  \"colors\": {{\n    \"accent\": \"#0078d4\"\n  }},\n  \"tokenColors\": {{\n  }},\n  \"fonts\": {{\n  }},\n  \"metrics\": {{\n  }},\n  \"editor\": {{\n  }},\n  \"layout\": {{\n  }}\n}}\n",
                            current.kind, current.name
                        );
                        std::fs::write(&path, body)?;
                    }
                    self.settings.set("workbench.theme", json!(text));
                    self.apply_theme(&text, window, cx);
                    self.open_file(path, None, window, cx);
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            self.log(format!("Operation failed: {e}"));
            self.show_panel(PanelView::Output, cx);
        }
    }
}
