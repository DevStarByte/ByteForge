//! Go to Symbol in Editor ("@" in the palette) and in Workspace ("#", Ctrl+T), from the language server.

use super::*;

/// A palette row that does nothing when picked (status text like "No symbols").
fn note(text: impl Into<String>) -> PaletteItem {
    PaletteItem::new(text, PaletteAction::GoToLine(usize::MAX))
}

impl Workbench {
    /// Where an LSP position is in `path`: converted through the open editor's text when the file is open
    /// (columns are UTF-16 there), else taken as is.
    fn symbol_point(&self, path: &Path, line: u32, character: u32, cx: &App) -> Point {
        match self.tabs.iter().find(|t| t.path.as_deref().is_some_and(|p| bf_core::paths::same_path(p, path))) {
            Some(tab) => tab.editor.read(cx).state.buffer.utf16_to_point(line as usize, character as usize),
            None => Point::new(line as usize, character as usize),
        }
    }

    /// Fills the open palette with symbols when it is in a symbol mode (`query` is for workspace symbols).
    pub(super) fn load_palette_symbols(&mut self, mode: Mode, query: String, cx: &mut Context<Self>) {
        let rx = match mode {
            Mode::Symbols => match self.active_editor().and_then(|e| e.read(cx).document_symbols()) {
                Some(rx) => rx,
                None => return self.set_palette_symbols(mode, vec![note("Symbols need a C# file and the C# language server")], cx),
            },
            Mode::WorkspaceSymbols => {
                let Some(lsp) = self.lsp.clone() else {
                    return self.set_palette_symbols(mode, vec![note("Symbols need the C# language server (open a folder with a .NET project)")], cx);
                };
                if query.is_empty() {
                    return self.set_palette_symbols(mode, Vec::new(), cx);
                }
                lsp.workspace_symbols(&query)
            }
            _ => return,
        };
        self.symbol_task = Some(cx.spawn(async move |this, cx| {
            let result = rx.recv_async().await;
            let _ = this.update(cx, |wb, cx| {
                let items = match result {
                    Ok(Ok(symbols)) => wb.symbol_items(mode, symbols, cx),
                    Ok(Err(e)) => vec![note(format!("The language server couldn't list symbols: {e}"))],
                    Err(_) => return,
                };
                wb.set_palette_symbols(mode, items, cx);
            });
        }));
    }

    fn set_palette_symbols(&mut self, mode: Mode, items: Vec<PaletteItem>, cx: &mut Context<Self>) {
        if let Some((palette, _)) = &self.palette
            && palette.read(cx).mode == mode
        {
            palette.update(cx, |p, cx| p.set_items(mode, items, cx));
        }
    }

    fn symbol_items(&self, mode: Mode, symbols: Vec<bf_lsp::Symbol>, cx: &App) -> Vec<PaletteItem> {
        let root = self.workspace.root();
        let items: Vec<PaletteItem> = symbols
            .into_iter()
            // csharp-ls puts the whole outline under a node for the file itself.
            .filter(|s| s.kind != "file")
            .map(|s| {
                let at = self.symbol_point(&s.path, s.range.start.line, s.range.start.character, cx);
                let mut item = PaletteItem::new(
                    if mode == Mode::Symbols { format!("{}{}", "  ".repeat(s.depth.saturating_sub(1)), s.name) } else { s.name.clone() },
                    PaletteAction::GoTo(s.path.clone(), at),
                );
                item.search = Some(s.name.clone());
                item.hint = Some(s.kind.to_owned());
                item.detail = if mode == Mode::Symbols {
                    s.detail.clone()
                } else {
                    let file = s.path.strip_prefix(root.as_deref().unwrap_or(Path::new(""))).unwrap_or(&s.path).display().to_string();
                    Some(match &s.container {
                        Some(c) => format!("{c} · {file}"),
                        None => file,
                    })
                };
                item
            })
            .collect();
        if items.is_empty() { vec![note("No symbols found")] } else { items }
    }
}
