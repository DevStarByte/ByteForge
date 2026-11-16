# ByteForge roadmap: an IDE you can work in every day

Worked through top to bottom. Tick items off as they land. What the first roadmap built is in the README and
the git history.

## 1. Make what's there trustworthy

- [ ] Dogfooding: use ByteForge for real work and fix what turns up. Every feature below was clicked through headlessly
      (real mouse and key events on the real workbench, against csharp-ls and dotnet where needed) and the bugs that
      turned up are fixed with regression tests. Still to look at by hand, where tests can't see:
      - Looks: diff spacer rows and stripes, merge editor's three panes, toolbars at narrow widths, blame gutter width,
        the floating tab while dragging and the drop highlights, breadcrumbs per group with tabs at the bottom,
        the Refactor popup's position and scrolling.
      - Real dialogs: Revert Change, the two-file refactoring confirmation, deleting a secret (Enter/Escape).
      - Clicking the "Accept Current · Incoming · Both" note after `<<<<<<<` (hit areas, with horizontal scroll).
      - Ctrl+. in a conflict with csharp-ls running (server entries are appended while the menu is open).
      - A deleted secret's row disappears; Shift+F5 during a test run leaves no test "running".
      - Refactorings never applied in tests: Extract Local Function, Introduce Parameter variants, Sort Usings;
        and all of them with `"csharp.server": "roslyn"`.
      Known, not fixed yet: Revert Change on an untracked file empties it instead of deleting it; saving secrets
      drops comments in secrets.json; Run All stops at the first test project that fails to build; breadcrumb
      folder/file segments aren't clickable (no dropdowns).
- [ ] Branch picker: clicking a branch doesn't switch to it. Tests now click the real row and the branch switches;
      a switch git refuses (local changes in the way) asks, with "Stash Changes and Switch", and a switch while git
      is busy says so in the Output. Still to confirm in the app; if it happens again, the Output says why.
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
- [x] Diff editor: edit the working-tree side in place. Side by side, both editors now hold their file's own text and the rows that keep them aligned are spacer rows, not text; in a working-tree diff the right side is editable, the diff follows the typing, an open tab of the file gets the edits and Ctrl+S (or auto save) writes the file. Inline stays read-only (switching to it saves first); Stage/Revert Change save first too.
- [x] Blame for every line: "Git: Toggle Blame Gutter" adds a column with each run of lines' commit, author and age (click: that commit's change). Merge editor ("Merge Conflict: Open Merge Editor"): Current and Incoming as git has them, read-only, above the Result, which is the file's own editor; the toolbar counts conflicts, steps through them, accepts a side and marks the file resolved (save and stage).
- [x] F5 in a container: "Start Debugging in Container" (Run menu) builds the startup project's image from its Dockerfile (as Debug when it takes BUILD_CONFIGURATION; the context is found as Visual Studio's Dockerfiles need it), starts the container with its ports published, attaches the debugger to the app in it, and removes the container when the session ends.

Not possible with today's tools: Change Signature with csharp-ls (needs a dialog it doesn't have; see
the Roslyn server above), and debugging in Alpine (musl) images (netcoredbg has no musl release).

## 4. Later

- [ ] Code coverage in the gutter.
- [ ] Git stash.
- [ ] Sticky scroll (the current type and member pinned at the top of the editor).
- [ ] An AI assistant panel.
