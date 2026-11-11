//! Binds editor actions to the view.

use bf_core::conflicts::Resolution;

use super::refactor::{self, Refactoring};
use super::*;

fn only(r: Refactoring) -> refactor::MenuMode {
    refactor::MenuMode::Only(r)
}

impl EditorView {
    pub(super) fn register_actions(&self, d: gpui::Stateful<gpui::Div>, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        d.on_action(cx.listener(|v, _: &MoveLeft, _, cx| v.mv(cx, |s| s.move_left(false, false))))
            .on_action(cx.listener(|v, _: &MoveRight, _, cx| v.mv(cx, |s| s.move_right(false, false))))
            .on_action(cx.listener(|v, _: &SelectLeft, _, cx| v.mv(cx, |s| s.move_left(false, true))))
            .on_action(cx.listener(|v, _: &SelectRight, _, cx| v.mv(cx, |s| s.move_right(false, true))))
            .on_action(cx.listener(|v, _: &MoveWordLeft, _, cx| v.mv(cx, |s| s.move_left(true, false))))
            .on_action(cx.listener(|v, _: &MoveWordRight, _, cx| v.mv(cx, |s| s.move_right(true, false))))
            .on_action(cx.listener(|v, _: &SelectWordLeft, _, cx| v.mv(cx, |s| s.move_left(true, true))))
            .on_action(cx.listener(|v, _: &SelectWordRight, _, cx| v.mv(cx, |s| s.move_right(true, true))))
            .on_action(cx.listener(|v, _: &MoveUp, _, cx| {
                if !v.action_menu_nav(-1, cx) && !v.completion_nav(-1, cx) {
                    v.mv(cx, |s| s.move_vertical(-1, false))
                }
            }))
            .on_action(cx.listener(|v, _: &MoveDown, _, cx| {
                if !v.action_menu_nav(1, cx) && !v.completion_nav(1, cx) {
                    v.mv(cx, |s| s.move_vertical(1, false))
                }
            }))
            .on_action(cx.listener(|v, _: &SelectUp, _, cx| v.mv(cx, |s| s.move_vertical(-1, true))))
            .on_action(cx.listener(|v, _: &SelectDown, _, cx| v.mv(cx, |s| s.move_vertical(1, true))))
            .on_action(cx.listener(|v, _: &MoveHome, _, cx| v.mv(cx, |s| s.move_home(false))))
            .on_action(cx.listener(|v, _: &MoveEnd, _, cx| v.mv(cx, |s| s.move_end(false))))
            .on_action(cx.listener(|v, _: &SelectHome, _, cx| v.mv(cx, |s| s.move_home(true))))
            .on_action(cx.listener(|v, _: &SelectEnd, _, cx| v.mv(cx, |s| s.move_end(true))))
            .on_action(cx.listener(|v, _: &AddCursorAbove, _, cx| v.mv(cx, |s| { s.add_cursor_vertical(false); })))
            .on_action(cx.listener(|v, _: &AddCursorBelow, _, cx| v.mv(cx, |s| { s.add_cursor_vertical(true); })))
            .on_action(cx.listener(|v, _: &AddNextMatch, _, cx| v.mv(cx, |s| { s.add_next_match(); })))
            .on_action(cx.listener(|v, _: &SelectAllMatches, _, cx| v.mv(cx, |s| { s.select_all_matches(); })))
            .on_action(cx.listener(|v, _: &MoveDocStart, _, cx| v.mv(cx, |s| s.set_caret(Point::default(), false))))
            .on_action(cx.listener(|v, _: &MoveDocEnd, _, cx| v.mv(cx, |s| s.set_caret(s.buffer.end(), false))))
            .on_action(cx.listener(|v, _: &SelectDocStart, _, cx| v.mv(cx, |s| s.set_caret(Point::default(), true))))
            .on_action(cx.listener(|v, _: &SelectDocEnd, _, cx| v.mv(cx, |s| s.set_caret(s.buffer.end(), true))))
            .on_action(cx.listener(|v, _: &PageUp, _, cx| {
                if !v.completion_nav(-8, cx) {
                    let n = v.page_lines();
                    let t = (v.scroll_target.0, v.scroll_target.1 - n as f32 * v.layout.line_height);
                    v.scroll_to(t, v.smooth_scrolling);
                    v.mv(cx, |s| s.move_vertical(-n, false));
                }
            }))
            .on_action(cx.listener(|v, _: &PageDown, _, cx| {
                if !v.completion_nav(8, cx) {
                    let n = v.page_lines();
                    let t = (v.scroll_target.0, v.scroll_target.1 + n as f32 * v.layout.line_height);
                    v.scroll_to(t, v.smooth_scrolling);
                    v.mv(cx, |s| s.move_vertical(n, false));
                }
            }))
            .on_action(cx.listener(|v, _: &SelectPageUp, _, cx| {
                let n = v.page_lines();
                v.mv(cx, |s| s.move_vertical(-n, true))
            }))
            .on_action(cx.listener(|v, _: &SelectPageDown, _, cx| {
                let n = v.page_lines();
                v.mv(cx, |s| s.move_vertical(n, true))
            }))
            .on_action(cx.listener(|v, _: &ScrollLineUp, _, cx| {
                let t = (v.scroll.0, v.scroll.1 - v.layout.line_height);
                v.scroll_to(t, false);
                cx.notify();
            }))
            .on_action(cx.listener(|v, _: &ScrollLineDown, _, cx| {
                let t = (v.scroll.0, v.scroll.1 + v.layout.line_height);
                v.scroll_to(t, false);
                cx.notify();
            }))
            .on_action(cx.listener(|v, _: &Backspace, _, cx| v.edit(cx, |s| s.backspace(false))))
            .on_action(cx.listener(|v, _: &DeleteWordLeft, _, cx| v.edit(cx, |s| s.backspace(true))))
            .on_action(cx.listener(|v, _: &Delete, _, cx| v.edit(cx, |s| s.delete_forward(false))))
            .on_action(cx.listener(|v, _: &DeleteWordRight, _, cx| v.edit(cx, |s| s.delete_forward(true))))
            .on_action(cx.listener(|v, _: &Newline, _, cx| {
                if !v.commit_action(None, cx) && !v.commit_completion(cx) {
                    v.edit(cx, |s| s.newline())
                }
            }))
            .on_action(cx.listener(|v, _: &Tab, _, cx| {
                if !v.commit_action(None, cx) && !v.commit_completion(cx) && !v.next_snippet_field(1, cx) {
                    v.edit(cx, |s| s.tab())
                }
            }))
            .on_action(cx.listener(|v, _: &Outdent, _, cx| {
                if !v.next_snippet_field(-1, cx) {
                    v.edit(cx, |s| s.indent_lines(false))
                }
            }))
            .on_action(cx.listener(|v, _: &SelectAll, _, cx| v.mv(cx, |s| s.select_all())))
            .on_action(cx.listener(|v, _: &Copy, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(v.state.copy_text()))))
            .on_action(cx.listener(|v, _: &Cut, _, cx| {
                if v.read_only {
                    return;
                }
                let text = v.state.cut();
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                v.after_edit(cx);
            }))
            .on_action(cx.listener(|v, _: &Paste, _, cx| {
                if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
                    v.edit(cx, |s| s.paste(&text));
                }
            }))
            .on_action(cx.listener(|v, _: &Undo, _, cx| v.edit(cx, |s| s.undo())))
            .on_action(cx.listener(|v, _: &Redo, _, cx| v.edit(cx, |s| s.redo())))
            .on_action(cx.listener(|v, _: &DuplicateLines, _, cx| v.edit(cx, |s| s.duplicate_lines())))
            .on_action(cx.listener(|v, _: &DeleteLines, _, cx| v.edit(cx, |s| s.delete_lines())))
            .on_action(cx.listener(|v, _: &MoveLinesUp, _, cx| v.edit(cx, |s| s.move_lines(false))))
            .on_action(cx.listener(|v, _: &MoveLinesDown, _, cx| v.edit(cx, |s| s.move_lines(true))))
            .on_action(cx.listener(|v, _: &ToggleComment, _, cx| v.edit(cx, |s| s.toggle_comment())))
            .on_action(cx.listener(|v, _: &Find, w, cx| v.open_find(false, w, cx)))
            .on_action(cx.listener(|v, _: &Replace, w, cx| v.open_find(true, w, cx)))
            .on_action(cx.listener(|v, _: &FindNext, _, cx| v.find_step(true, cx)))
            .on_action(cx.listener(|v, _: &FindPrevious, _, cx| v.find_step(false, cx)))
            .on_action(cx.listener(|v, _: &TriggerCompletion, _, cx| v.request_completion(cx)))
            .on_action(cx.listener(|v, _: &GoToDefinition, _, cx| {
                let caret = v.state.caret();
                v.go_to_definition_at(caret, cx);
            }))
            .on_action(cx.listener(|v, _: &QuickFix, _, cx| v.request_code_actions(refactor::MenuMode::QuickFix, cx)))
            .on_action(cx.listener(|v, _: &Refactor, _, cx| v.refactor(refactor::MenuMode::Refactor, cx)))
            .on_action(cx.listener(|v, _: &ExtractMethod, _, cx| v.refactor(only(Refactoring::ExtractMethod), cx)))
            .on_action(cx.listener(|v, _: &ExtractLocalFunction, _, cx| v.refactor(only(Refactoring::ExtractLocalFunction), cx)))
            .on_action(cx.listener(|v, _: &IntroduceVariable, _, cx| v.refactor(only(Refactoring::IntroduceVariable), cx)))
            .on_action(cx.listener(|v, _: &IntroduceConstant, _, cx| v.refactor(only(Refactoring::IntroduceConstant), cx)))
            .on_action(cx.listener(|v, _: &IntroduceField, _, cx| v.refactor(only(Refactoring::IntroduceField), cx)))
            .on_action(cx.listener(|v, _: &IntroduceParameter, _, cx| v.refactor(only(Refactoring::IntroduceParameter), cx)))
            .on_action(cx.listener(|v, _: &InlineSymbol, _, cx| v.refactor(only(Refactoring::Inline), cx)))
            .on_action(cx.listener(|v, _: &ExtractInterface, _, cx| v.refactor(only(Refactoring::ExtractInterface), cx)))
            .on_action(cx.listener(|v, _: &ExtractBaseClass, _, cx| v.refactor(only(Refactoring::ExtractBaseClass), cx)))
            .on_action(cx.listener(|v, _: &MoveTypeToFile, _, cx| v.refactor(only(Refactoring::MoveTypeToFile), cx)))
            .on_action(cx.listener(|v, _: &RemoveAndSortUsings, _, cx| v.refactor(only(Refactoring::RemoveAndSortUsings), cx)))
            .on_action(cx.listener(|v, _: &SortUsings, _, cx| v.sort_usings(cx)))
            .on_action(cx.listener(|v, _: &AcceptCurrentChange, _, cx| v.accept_conflict(Resolution::Current, cx)))
            .on_action(cx.listener(|v, _: &AcceptIncomingChange, _, cx| v.accept_conflict(Resolution::Incoming, cx)))
            .on_action(cx.listener(|v, _: &AcceptBothChanges, _, cx| v.accept_conflict(Resolution::Both, cx)))
            .on_action(cx.listener(|v, _: &NextConflict, _, cx| v.go_to_conflict(true, cx)))
            .on_action(cx.listener(|v, _: &PreviousConflict, _, cx| v.go_to_conflict(false, cx)))
            .on_action(cx.listener(|v, _: &ShowLineCommit, _, cx| v.show_line_commit(cx)))
            .on_action(cx.listener(|v, _: &FindReferences, _, cx| v.find_references(cx)))
            .on_action(cx.listener(|v, _: &Fold, _, cx| v.fold_at_caret(cx)))
            .on_action(cx.listener(|v, _: &Unfold, _, cx| v.unfold_at_caret(cx)))
            .on_action(cx.listener(|v, _: &FoldAll, _, cx| v.fold_all(cx)))
            .on_action(cx.listener(|v, _: &UnfoldAll, _, cx| v.unfold_all(cx)))
            .on_action(cx.listener(|v, _: &GoToImplementation, _, cx| v.go_to_related(true, cx)))
            .on_action(cx.listener(|v, _: &GoToTypeDefinition, _, cx| v.go_to_related(false, cx)))
            .on_action(cx.listener(|v, _: &RenameSymbol, _, cx| v.request_rename(cx)))
            .on_action(cx.listener(|v, _: &ShowSignatureHelp, _, cx| v.request_signature_help(cx)))
            .on_action(cx.listener(|v, _: &ToggleBreakpoint, _, cx| {
                if v.glyph_margin {
                    let line = v.state.caret().line;
                    v.toggle_breakpoint(line, cx);
                } else {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|v, _: &Cancel, w, cx| {
                if v.action_menu.take().is_some()
                    || v.context_menu.take().is_some()
                    || v.completion.take().is_some()
                    || v.hover.take().is_some()
                    || v.signature.take().is_some()
                    || v.snippet_session.take().is_some()
                {
                    v.hover_pending = None;
                    cx.notify();
                } else if v.find.is_some() {
                    v.close_find(w, cx);
                } else if v.state.has_multiple_cursors() {
                    v.state.collapse_cursors();
                    cx.notify();
                } else if v.state.has_selection() {
                    v.state.clear_selection();
                    cx.notify();
                } else {
                    cx.propagate();
                }
            }))
    }
}
