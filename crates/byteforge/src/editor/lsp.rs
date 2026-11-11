//! Language-server features: diagnostics, navigation, rename, signature help, hover, code actions and text edits.

use super::*;

impl EditorView {
    pub fn attach_lsp(&mut self, lsp: LspClient, cx: &mut Context<Self>) {
        if let Some(path) = &self.path {
            self.lsp_version = 1;
            lsp.did_open(path, self.lsp_version, &self.state.buffer.text());
            self.lsp = Some(lsp);
            self.schedule_semantic(Duration::from_millis(100), cx);
            self.schedule_inlay_hints(Duration::from_millis(300), cx);
        }
    }

    /// The file was renamed or moved: the language server forgets the old path and opens the new one.
    pub fn set_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let lsp = self.lsp.clone();
        self.detach_lsp();
        self.path = Some(path);
        if let Some(lsp) = lsp {
            self.attach_lsp(lsp, cx);
        }
        cx.notify();
    }

    pub fn detach_lsp(&mut self) {
        if let (Some(lsp), Some(path)) = (self.lsp.take(), &self.path) {
            lsp.did_close(path);
        }
    }

    pub fn set_lsp_diagnostics(&mut self, diagnostics: Vec<bf_lsp::Diagnostic>, cx: &mut Context<Self>) {
        let b = &self.state.buffer;
        self.diagnostics = diagnostics
            .into_iter()
            .map(|d| Diagnostic {
                start: b.utf16_to_point(d.range.start.line as usize, d.range.start.character as usize),
                end: b.utf16_to_point(d.range.end.line as usize, d.range.end.character as usize),
                severity: d.severity,
                message: d.message,
                code: d.code,
            })
            .collect();
        self.diagnostics.sort_by_key(|d| d.start);
        cx.emit(EditorEvent::DiagnosticsChanged);
        cx.notify();
    }

    /// Sets diagnostics from another source (build output). One-character ranges grow to the word there.
    pub fn set_diagnostics(&mut self, mut diagnostics: Vec<Diagnostic>, cx: &mut Context<Self>) {
        let lines = self.state.buffer.line_count();
        for d in &mut diagnostics {
            d.start.line = d.start.line.min(lines.saturating_sub(1));
            d.start.col = d.start.col.min(self.state.buffer.line_len(d.start.line));
            if d.end.line == d.start.line && d.end.col <= d.start.col + 1 {
                let (_, word_end) = self.state.word_at(d.start);
                d.end = if word_end.col > d.start.col { word_end } else { Point::new(d.start.line, d.start.col + 1) };
            }
        }
        diagnostics.sort_by_key(|d| d.start);
        self.diagnostics = diagnostics;
        cx.emit(EditorEvent::DiagnosticsChanged);
        cx.notify();
    }

        pub fn did_save(&self) {
        if let (Some(lsp), Some(path)) = (&self.lsp, &self.path) {
            lsp.did_save(path, Some(&self.state.buffer.text()));
        }
    }

    pub(super) fn lsp_position(&self, p: Point) -> Position {
        let (line, character) = self.state.buffer.point_to_utf16(p);
        Position { line: line as u32, character: character as u32 }
    }

    pub fn go_to_definition_at(&mut self, p: Point, cx: &mut Context<Self>) {
        let (Some(lsp), Some(path)) = (self.lsp.clone(), self.path.clone()) else { return };
        let rx = lsp.definition(&path, self.lsp_position(p));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(locations)) = rx.recv_async().await
                && let Some(loc) = locations.into_iter().next()
            {
                let _ = this.update(cx, |_, cx| {
                    cx.emit(EditorEvent::Navigate(loc.path, Point::new(loc.range.start.line as usize, loc.range.start.character as usize)));
                });
            }
        })
        .detach();
    }

    /// Find All References for the symbol at the caret; the result goes to the workbench's References panel.
    pub fn find_references(&mut self, cx: &mut Context<Self>) {
        let (Some(lsp), Some(path)) = (self.lsp.clone(), self.path.clone()) else {
            cx.emit(EditorEvent::Log("Find All References needs the C# language server (open a .cs file in a project).".into()));
            return;
        };
        let caret = self.state.caret();
        let (start, end) = self.state.word_at(caret);
        let symbol = self.state.buffer.slice(start, end);
        let rx = lsp.references(&path, self.lsp_position(caret));
        cx.spawn(async move |this, cx| {
            let result = rx.recv_async().await;
            let _ = this.update(cx, |_, cx| match result {
                Ok(Ok(locations)) => cx.emit(EditorEvent::Locations { title: format!("references to '{symbol}'"), locations }),
                Ok(Err(e)) => cx.emit(EditorEvent::Log(format!("Find All References failed: {e:#}"))),
                Err(_) => {}
            });
        })
        .detach();
    }

    /// Go to Implementation (`implementation: true`) or Go to Type Definition: jumps to a single result,
    /// lists several in the References panel.
    pub fn go_to_related(&mut self, implementation: bool, cx: &mut Context<Self>) {
        let what = if implementation { "implementations" } else { "type definition" };
        let (Some(lsp), Some(path)) = (self.lsp.clone(), self.path.clone()) else {
            cx.emit(EditorEvent::Log(format!("Finding {what} needs the C# language server (open a .cs file in a project).")));
            return;
        };
        let caret = self.state.caret();
        let (start, end) = self.state.word_at(caret);
        let symbol = self.state.buffer.slice(start, end);
        let position = self.lsp_position(caret);
        let rx = if implementation { lsp.implementation(&path, position) } else { lsp.type_definition(&path, position) };
        cx.spawn(async move |this, cx| {
            let result = rx.recv_async().await;
            let _ = this.update(cx, |_, cx| match result {
                Ok(Ok(locations)) if locations.is_empty() => {
                    cx.emit(EditorEvent::Log(format!("No {what} found for '{symbol}' (types from the .NET libraries have no source here).")))
                }
                Ok(Ok(mut locations)) if locations.len() == 1 => {
                    let loc = locations.remove(0);
                    cx.emit(EditorEvent::Navigate(loc.path, Point::new(loc.range.start.line as usize, loc.range.start.character as usize)));
                }
                Ok(Ok(locations)) => cx.emit(EditorEvent::Locations { title: format!("{what} of '{symbol}'"), locations }),
                Ok(Err(e)) => cx.emit(EditorEvent::Log(format!("Finding {what} failed: {e:#}"))),
                Err(_) => {}
            });
        })
        .detach();
    }

    /// The file's symbols (outline) from the language server, or `None` without one.
    pub fn document_symbols(&self) -> Option<flume::Receiver<anyhow::Result<Vec<bf_lsp::Symbol>>>> {
        let (Some(lsp), Some(path)) = (&self.lsp, &self.path) else { return None };
        Some(lsp.document_symbols(path))
    }

    /// F2: asks the workbench for the new name of the identifier at the caret.
    pub fn request_rename(&mut self, cx: &mut Context<Self>) {
        if self.lsp.is_none() || self.read_only {
            cx.emit(EditorEvent::Log("Rename Symbol needs the C# language server (open a .cs file in a project).".into()));
            return;
        }
        let caret = self.state.caret();
        let (start, end) = self.state.word_at(caret);
        let symbol = self.state.buffer.slice(start, end);
        if !symbol.chars().any(|c| c.is_alphanumeric() || c == '_') {
            cx.emit(EditorEvent::Log("Put the caret on a name to rename it.".into()));
            return;
        }
        cx.emit(EditorEvent::RenameRequested { symbol, at: start });
    }

    /// Asks the server to rename the symbol at `at` to `new_name`; the edits go to the workbench.
    pub fn rename_symbol(&mut self, at: Point, new_name: String, cx: &mut Context<Self>) {
        let (Some(lsp), Some(path)) = (self.lsp.clone(), self.path.clone()) else { return };
        let rx = lsp.rename(&path, self.lsp_position(at), &new_name);
        cx.spawn(async move |this, cx| {
            let result = rx.recv_async().await;
            let _ = this.update(cx, |_, cx| match result {
                Ok(Ok(edit)) if edit.is_empty() => cx.emit(EditorEvent::Log(format!("Nothing to rename to '{new_name}' here."))),
                Ok(Ok(edit)) => cx.emit(EditorEvent::ApplyWorkspaceEdit(Box::new(edit))),
                Ok(Err(e)) => cx.emit(EditorEvent::Log(format!("Rename failed: {e:#}"))),
                Err(_) => {}
            });
        })
        .detach();
    }

    /// Asks for parameter hints at the caret; closes them when the caret left the call.
    pub(super) fn request_signature_help(&mut self, cx: &mut Context<Self>) {
        let (Some(lsp), Some(path)) = (self.lsp.clone(), self.path.clone()) else { return };
        let rx = lsp.signature_help(&path, self.lsp_position(self.state.caret()));
        self.signature_task = Some(cx.spawn(async move |this, cx| {
            let Ok(result) = rx.recv_async().await else { return };
            let _ = this.update(cx, |view, cx| {
                match result {
                    Ok(Some(help)) => {
                        // Keep the overload the user picked while the list stays the same.
                        let selected = match &view.signature {
                            Some(old) if old.help.signatures.len() == help.signatures.len() => old.selected,
                            _ => help.active_signature,
                        };
                        view.signature = Some(SignatureState { help, selected });
                    }
                    _ => view.signature = None,
                }
                cx.notify();
            });
        }));
    }

    pub(super) fn cycle_signature(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(s) = &mut self.signature {
            let n = s.help.signatures.len() as isize;
            s.selected = ((s.selected as isize + delta).rem_euclid(n)) as usize;
            cx.notify();
        }
    }

    /// The word under `p` (at least one character), which a hover popup belongs to.
    pub(super) fn hover_range(&self, p: Point) -> (Point, Point) {
        let (start, end) = self.state.word_at(p);
        if start.line == end.line && start.col < end.col && start.col <= p.col && p.col < end.col {
            (start, end)
        } else {
            (p, Point::new(p.line, p.col + 1))
        }
    }

    pub(super) fn diagnostics_in(&self, range: (Point, Point)) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics.iter().filter(move |d| d.start < range.1 && (d.end > range.0 || (d.start == d.end && d.start.line == range.0.line)))
    }

    /// Shows a hover for the word under `p` after a short delay; moving off the word hides it.
    pub(super) fn request_hover(&mut self, p: Point, cx: &mut Context<Self>) {
        let inside = |r: (Point, Point)| p >= r.0 && p < r.1;
        if self.hover.as_ref().is_some_and(|h| inside(h.range)) || self.hover_pending.is_some_and(inside) {
            return;
        }
        self.clear_hover(cx);
        let range = self.hover_range(p);
        let server = match (&self.lsp, &self.path) {
            (Some(lsp), Some(path)) => Some((lsp.clone(), path.clone(), self.lsp_position(p))),
            _ if self.diagnostics_in(range).next().is_some() => None,
            _ => return,
        };
        self.hover_pending = Some(range);
        self.hover_task = Some(cx.spawn(async move |this, cx| {
            // Ask only once the pointer rests on the word (dropping the task cancels this).
            cx.background_executor().timer(Duration::from_millis(400)).await;
            let text = match server {
                Some((lsp, path, position)) => lsp.hover(&path, position).recv_async().await.ok().and_then(Result::ok).flatten().unwrap_or_default(),
                None => String::new(),
            };
            let _ = this.update(cx, |view, cx| {
                if view.hover_pending != Some(range) {
                    return;
                }
                view.hover_pending = None;
                if !text.trim().is_empty() || view.diagnostics_in(range).next().is_some() {
                    view.hover = Some(Hover { range, text });
                    cx.notify();
                }
            });
        }));
    }

    // ───────────── code actions ─────────────

    /// Asks the language server for the code actions at the caret (or selection) and lists them as `mode`
    /// says. For Ctrl+. on a line with a problem the problem's range is used, so the cursor only has to be
    /// on that line.
    pub(super) fn request_code_actions(&mut self, mode: refactor::MenuMode, cx: &mut Context<Self>) {
        // In a merge conflict, Ctrl+. offers its resolutions first (in any file, server or not).
        let conflict = if mode == refactor::MenuMode::QuickFix { self.conflict_entries() } else { Vec::new() };
        let (Some(lsp), Some(path)) = (self.lsp.clone(), self.path.clone()) else {
            if !conflict.is_empty() {
                self.completion = None;
                self.context_menu = None;
                self.clear_hover(cx);
                let at = self.state.caret();
                self.action_menu = Some(ActionMenu { at, items: Some(conflict), selected: 0, mode, empty: None });
                cx.notify();
            }
            return;
        };
        let caret = self.state.caret();
        let (mut start, mut end) = if self.state.has_selection() { self.state.selection() } else { (caret, caret) };
        let context: Vec<&Diagnostic> = match mode {
            refactor::MenuMode::QuickFix => {
                let on_caret: Vec<&Diagnostic> = self.diagnostics.iter().filter(|d| d.start <= end && d.end >= start).collect();
                let context = if on_caret.is_empty() && !self.state.has_selection() {
                    self.diagnostics.iter().filter(|d| d.start.line <= caret.line && d.end.line >= caret.line).collect()
                } else {
                    on_caret
                };
                if !self.state.has_selection()
                    && let Some(first) = context.first()
                {
                    (start, end) = (first.start, first.end);
                }
                context
            }
            // The unused usings are reported as CS8019; their fix is "Remove unnecessary usings".
            refactor::MenuMode::Only(refactor::Refactoring::RemoveAndSortUsings) => {
                let unused: Vec<&Diagnostic> = self.diagnostics.iter().filter(|d| matches!(d.code.as_deref(), Some("CS8019" | "IDE0005"))).collect();
                if let (Some(first), Some(last)) = (unused.first(), unused.last()) {
                    (start, end) = (first.start, last.end);
                }
                unused
            }
            _ => Vec::new(),
        };
        let range = bf_lsp::Range { start: self.lsp_position(start), end: self.lsp_position(end) };
        let diagnostics: Vec<bf_lsp::Diagnostic> = context
            .iter()
            .map(|d| bf_lsp::Diagnostic {
                range: bf_lsp::Range { start: self.lsp_position(d.start), end: self.lsp_position(d.end) },
                severity: d.severity,
                message: d.message.clone(),
                code: d.code.clone(),
            })
            .collect();
        self.completion = None;
        self.context_menu = None;
        self.clear_hover(cx);
        self.action_menu = Some(ActionMenu { at: start, items: None, selected: 0, mode, empty: None });
        let rx = lsp.code_actions(&path, range, &diagnostics);
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = rx.recv_async().await;
            let _ = this.update(cx, |view, cx| {
                if view.action_menu.as_ref().is_none_or(|m| m.mode != mode) {
                    return;
                }
                let items = match result {
                    Ok(Ok(items)) => items,
                    _ => Vec::new(),
                };
                if let Some((mut items, empty)) = view.filter_actions(mode, items, cx)
                    && let Some(menu) = view.action_menu.as_mut()
                {
                    items.splice(0..0, conflict);
                    menu.items = Some(items);
                    menu.empty = empty;
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn action_menu_nav(&mut self, delta: isize, cx: &mut Context<Self>) -> bool {
        let Some(menu) = &mut self.action_menu else { return false };
        let n = menu.items.as_ref().map_or(0, Vec::len) as isize;
        if n > 0 {
            menu.selected = (menu.selected as isize + delta).clamp(0, n - 1) as usize;
        }
        cx.notify();
        true
    }

    pub(super) fn commit_action(&mut self, ix: Option<usize>, cx: &mut Context<Self>) -> bool {
        let Some(menu) = self.action_menu.take() else { return false };
        self.action_task = None;
        let ix = ix.unwrap_or(menu.selected);
        if let Some(action) = menu.items.and_then(|items| items.into_iter().nth(ix)).filter(|a| a.disabled.is_none())
            && !self.run_builtin_entry(&action, cx)
        {
            let then = if menu.mode == refactor::MenuMode::Only(refactor::Refactoring::RemoveAndSortUsings) { AfterApply::SortUsings } else { AfterApply::Nothing };
            cx.emit(EditorEvent::ApplyCodeAction(Box::new(action), then));
        }
        cx.notify();
        true
    }

    /// Replaces the text with `new` (a formatter's output) as one undo step, changing only the part that
    /// differs so the caret, scroll position and breakpoints stay where they were.
    pub fn replace_text_minimal(&mut self, new: &str, cx: &mut Context<Self>) {
        let new: Vec<char> = new.replace("\r\n", "\n").chars().collect();
        let old: Vec<char> = self.state.buffer.text().chars().collect();
        let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        let suffix = old[prefix..].iter().rev().zip(new[prefix..].iter().rev()).take_while(|(a, b)| a == b).count();
        if prefix == old.len() && prefix == new.len() {
            return;
        }
        let start = self.state.buffer.char_to_point(prefix);
        let end = self.state.buffer.char_to_point(old.len() - suffix);
        let inserted: String = new[prefix..new.len() - suffix].iter().collect();
        let caret = self.state.caret();
        self.state.buffer.undo_stack().begin_group();
        self.state.buffer.replace(start, end, &inserted);
        self.state.buffer.undo_stack().end_group();
        self.state.set_caret(self.state.buffer.clamp(caret), false);
        self.after_edit(cx);
    }

    /// Applies language-server text edits (UTF-16 ranges) as one undo step.
    pub fn apply_text_edits(&mut self, edits: &[bf_lsp::TextEdit], cx: &mut Context<Self>) {
        if edits.is_empty() {
            return;
        }
        let b = &self.state.buffer;
        let mut edits: Vec<(Point, Point, &str)> = edits
            .iter()
            .map(|e| {
                let start = b.utf16_to_point(e.range.start.line as usize, e.range.start.character as usize);
                let end = b.utf16_to_point(e.range.end.line as usize, e.range.end.character as usize);
                (start, end, e.new_text.as_str())
            })
            .collect();
        // Back to front, so earlier positions stay valid.
        edits.sort_by_key(|e| std::cmp::Reverse(e.0));
        self.state.buffer.undo_stack().begin_group();
        for (start, end, text) in edits {
            self.state.buffer.replace(start, end, &text.replace("\r\n", "\n"));
        }
        self.state.buffer.undo_stack().end_group();
        self.after_edit(cx);
        cx.notify();
    }

    pub(super) fn clear_hover(&mut self, cx: &mut Context<Self>) {
        self.hover_pending = None;
        self.hover_task = None;
        self.hover_bounds.set(None);
        if self.hover.take().is_some() {
            cx.notify();
        }
    }

    // ───────────── completion ─────────────
}
