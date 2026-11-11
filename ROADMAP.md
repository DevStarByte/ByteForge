# ByteForge roadmap: an IDE you can work in every day

Worked through top to bottom. Tick items off as they land. What the first roadmap built is in the README and
the git history.

## 1. Make what's there trustworthy

- [ ] Dogfooding: use ByteForge for real work and fix what turns up. Built but never clicked through: the
      Refactor menu, diff editor, blame, merge conflicts, breadcrumbs, tab dragging, one file in both groups,
      the User Secrets page, live test progress.
- [ ] Branch picker: clicking a branch doesn't switch to it. Tests now show picking a branch switches it, and a switch that git refuses (local changes in the way) used to fail silently into the Output log; it now asks, with "Stash Changes and Switch". Still to confirm in the app that this was the cause (the mouse click on the row itself isn't covered by a test).
- [x] The Roslyn language server (the one VS Code's C# extension uses) next to csharp-ls: `"csharp.server": "roslyn"` or
      "C#: Install or Update Roslyn Language Server", which downloads it from Microsoft's NuGet feed, checks Microsoft's
      signature and switches to it. Opens the solution (or the projects), pulls diagnostics (analyzers included),
      inlay hints, code actions with kinds and Fix All. Needs .NET 10. Still open: Razor IntelliSense (needs the Razor
      extension the C# extension loads). Change Signature isn't offered by Roslyn's server either (VS Code shows its
      own dialog for it).

## 2. Everyday features

- [x] Search: regex and whole-word options in Find and in Search across files (Aa / ab / .* toggles, Alt+C/W/R in the find bar, `$1` groups in Replace, matches highlighted in the Search results).
- [x] Replace across files (Search view): a Replace field (chevron), each result previews the match struck out and its replacement, Replace on one line or All (asks first; open files change in their editor, unsaved; the rest on disk, CRLF and BOM kept).
- [x] Debugger: conditional breakpoints (netcoredbg evaluates them), hit counts (`5`, `>= 5`, `% 5`) and logpoints (`total = {total}`, printed to the Debug Console without stopping); ByteForge counts hits and prints logpoints itself, as netcoredbg has neither. Debug menu, right-click in the editor, marked in the gutter and the Breakpoints list, saved with the folder.
- [x] Debugger: watch expressions (WATCH in the Debug view, evaluated in the selected frame, saved with the folder), and the value of the hovered identifier or member chain (`order.Items.Count`) at the top of the hover while paused.
- [x] Word wrap: `editor.wordWrap` "on" or Alt+Z (View menu). Long lines continue on the next rows at the window's width; clicks, the caret, selections, squiggles, Up/Down and Page Up/Down follow the rows; no horizontal scrolling.
- [x] Auto save: `files.autoSave` "afterDelay" (`files.autoSaveDelay`, no formatting), "onFocusChange" (switching tabs), "onWindowChange" (also leaving the window).
- [x] Settings screen (Ctrl+,, Preferences → Settings): every setting grouped and searchable, with switches, choices and values, "changed" marks and Reset. Writes change only that key in settings.json, so its comments and formatting stay (this also fixed every other settings write). "Open Settings (JSON)" for the file.

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
