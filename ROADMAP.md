# ByteForge roadmap: an IDE you can work in every day

Worked through top to bottom. Tick items off as they land. What the first roadmap built is in the README and
the git history.

## 1. Make what's there trustworthy

- [ ] Dogfooding: use ByteForge for real work and fix what turns up. Built but never clicked through: the
      Refactor menu, diff editor, blame, merge conflicts, breadcrumbs, tab dragging, one file in both groups,
      the User Secrets page, live test progress.
- [ ] Branch picker: clicking a branch doesn't switch to it (needs a repro in the app).
- [ ] Use the Roslyn language server (the one VS Code's C# extension uses) instead of, or next to,
      csharp-ls: more refactorings (Change Signature), Razor IntelliSense.

## 2. Everyday features

- [x] Search: regex and whole-word options in Find and in Search across files (Aa / ab / .* toggles, Alt+C/W/R in the find bar, `$1` groups in Replace, matches highlighted in the Search results).
- [x] Replace across files (Search view): a Replace field (chevron), each result previews the match struck out and its replacement, Replace on one line or All (asks first; open files change in their editor, unsaved; the rest on disk, CRLF and BOM kept).
- [x] Debugger: conditional breakpoints (netcoredbg evaluates them), hit counts (`5`, `>= 5`, `% 5`) and logpoints (`total = {total}`, printed to the Debug Console without stopping); ByteForge counts hits and prints logpoints itself, as netcoredbg has neither. Debug menu, right-click in the editor, marked in the gutter and the Breakpoints list, saved with the folder.
- [x] Debugger: watch expressions (WATCH in the Debug view, evaluated in the selected frame, saved with the folder), and the value of the hovered identifier or member chain (`order.Items.Count`) at the top of the hover while paused.
- [ ] Word wrap.
- [x] Auto save: `files.autoSave` "afterDelay" (`files.autoSaveDelay`, no formatting), "onFocusChange" (switching tabs), "onWindowChange" (also leaving the window).
- [ ] Settings screen (today only settings.json).

## 3. Left over from the roadmap

- [ ] EF Core: show which migrations are applied (needs the database), and `dotnet ef dbcontext scaffold`.
- [ ] Diff editor: stage or revert one change, collapse unchanged regions, edit the working-tree side.
- [ ] Blame for every line in the gutter; a three-way merge editor.
- [ ] F5 in a container: build, start and debug the app's container.

Not possible with today's tools: Change Signature with csharp-ls (needs a dialog it doesn't have; see
the Roslyn server above), and debugging in Alpine (musl) images (netcoredbg has no musl release).

## 4. Later

- [ ] Code coverage in the gutter.
- [ ] Git stash.
- [ ] Sticky scroll (the current type and member pinned at the top of the editor).
- [ ] An AI assistant panel.
