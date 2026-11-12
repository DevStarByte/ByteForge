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

- [x] EF Core: "Update Database to Migration…" marks each migration applied or pending once EF has asked the database (the list shows at once from the source; when the database can't be reached, the Output says why). "Scaffold DbContext from Database…" asks for the connection string (kept out of the log) and the provider, and generates the entities and the DbContext into Models/.
- [x] Diff editor: Stage Change / Revert Change (working tree) and Unstage Change (staged) for the change the toolbar is on: the index or the file gets exactly that change, in its own line endings. "Collapse Unchanged" folds the long stretches without changes on both sides.
- [ ] Diff editor: edit the working-tree side in place (the panes hold filler lines to stay aligned, so this needs rows that aren't text).
- [x] Blame for every line: "Git: Toggle Blame Gutter" adds a column with each run of lines' commit, author and age (click: that commit's change). Merge editor ("Merge Conflict: Open Merge Editor"): Current and Incoming as git has them, read-only, above the Result, which is the file's own editor; the toolbar counts conflicts, steps through them, accepts a side and marks the file resolved (save and stage).
- [x] F5 in a container: "Start Debugging in Container" (Run menu) builds the startup project's image from its Dockerfile (as Debug when it takes BUILD_CONFIGURATION; the context is found as Visual Studio's Dockerfiles need it), starts the container with its ports published, attaches the debugger to the app in it, and removes the container when the session ends.

Not possible with today's tools: Change Signature with csharp-ls (needs a dialog it doesn't have; see
the Roslyn server above), and debugging in Alpine (musl) images (netcoredbg has no musl release).

## 4. Later

- [ ] Code coverage in the gutter.
- [ ] Git stash.
- [ ] Sticky scroll (the current type and member pinned at the top of the editor).
- [ ] An AI assistant panel.
