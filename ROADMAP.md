# ByteForge roadmap: an IDE you can work in every day

Worked through top to bottom. Tick items off as they land. What the first roadmap built is in the README and
the git history.

## 1. Make what's there trustworthy

- [ ] Dogfooding: use ByteForge for real work and fix what turns up. Every feature was clicked through headlessly
      (real mouse and key events on the real workbench, against csharp-ls and dotnet where needed), and checked by
      hand in the app (0.11.0): the diff and merge editors, conflict notes and Ctrl+. in a conflict, the blame
      gutter, tab dragging, breadcrumbs and their dropdowns, toolbars at narrow widths, the Refactor popup, the
      refactorings (Roslyn server too) and their two-file confirmation, Revert Change, deleting a user secret,
      stopping a test run, and the multi-line commit box. Nothing is left to check by hand; what stays open is
      using it for real work.
      Fixed since: Revert Change on an untracked file deletes it (and says so first); saving a secret edits only
      that key, so comments and nesting in secrets.json stay; Run All goes on past a test project that doesn't
      build (Shift+F5 ends the queue); every breadcrumb opens a dropdown (folder entries, siblings, symbols); the
      commit message box spans the side bar and takes several lines; text boxes scroll to the caret; Space types
      in the terminal; Problems lists build errors of closed C# files while a language server runs.
- [x] Branch picker: clicking a branch doesn't switch to it. Confirmed working in the app (and the status bar follows). A switch git refuses (local changes in the way) asks, with
      "Stash Changes and Switch", and a switch while git is busy says so in the Output.
- [x] The Roslyn language server (the one VS Code's C# extension uses) next to csharp-ls: `"csharp.server": "roslyn"` or
      "C#: Install or Update Roslyn Language Server", which downloads it from Microsoft's NuGet feed, checks Microsoft's
      signature and switches to it. Opens the solution (or the projects), pulls diagnostics (analyzers included),
      inlay hints, code actions with kinds and Fix All. Needs .NET 10. Razor IntelliSense came later (section 5). Was open: Razor IntelliSense (needs the Razor
      extension the C# extension loads). Change Signature isn't offered by Roslyn's server either (VS Code shows its
      own dialog for it).

- [x] README "Known limitations" matches the app: merge editor, blame, conditional breakpoints and watch
      expressions are built, so they are no longer listed as missing; what still is (fast-forward-only pull,
      Razor IntelliSense, Change Signature, Blazor WebAssembly and Alpine debugging, unsigned downloads) is.

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

## 4. First stable release (1.0)

Section 1 above is what still has to be checked by hand; this is what the release itself needs.

- [x] CI: clippy (warnings fail) and the whole test suite on Arch Linux, Windows and macOS as the first step of
      the pipeline (`.github/workflows/release.yml`), so nothing is built or released untested.
- [x] Privacy statement (docs/code-signing.md) lists every connection: the update check at start, modules,
      netcoredbg and the Roslyn server downloads.
- [x] CHANGELOG.md, with what changed per version; the 1.0 notes come from it.
- [x] Release notes: known limits (no Razor IntelliSense with the Roslyn server, no Change Signature, no
      debugging in Alpine images), in CHANGELOG.md under 1.0.0.
- [x] Tag `v1.0.0`. Signing (SignPath) and notarization follow in a later release.

## 5. What a C# developer misses from Rider or Visual Studio

- [x] Code Lens: "3 references" above types and members (`textDocument/codeLens`, resolved visible-first; click:
      the References panel) and Run Test / Debug Test (Run All / Debug All Tests on a class) above tests, on a row
      above the attributes. Asked again after edits, saves of C# files, when the server has loaded and on its
      `codeLens/refresh`. `"editor.codeLens"` turns it off.
- [x] Call Hierarchy (Shift+Alt+H: callers or callees) and Type Hierarchy (base types or derived types), in a
      Hierarchy panel tab: a tree whose branches are asked for when opened (`prepareCallHierarchy`,
      `incomingCalls` / `outgoingCalls`, `prepareTypeHierarchy`, `supertypes` / `subtypes`). Go menu and right-click;
      a click opens the caller at its call, anything else at its declaration.
- [x] Razor IntelliSense (completion, hover, go to definition, diagnostics in `.razor` / `.cshtml`) with the Roslyn
      server: it's installed from nuget.org (`roslyn-language-server.<platform>`), whose package carries the Razor
      extension; the server serves Razor files itself (cohosting). Its HTML requests (`razor/updateHtml` and
      forwarded `textDocument/*`) are answered without HTML results, as there is no HTML language server yet.
- [x] Blazor WebAssembly debugging: F5 on a standalone WebAssembly app (`Microsoft.NET.Sdk.BlazorWebAssembly`)
      uses ByteForge's own debug adapter (`byteforge --blazor-wasm-adapter`, DAP like netcoredbg): it runs the
      app, opens it in Edge/Chrome/Chromium with remote debugging, and talks the DevTools protocol through
      Microsoft's WebAssembly debug proxy (BrowserDebugHost, from the WebAssembly SDK pack of the app's .NET
      version; .NET 11's gateway no longer starts it). Breakpoints in .razor and .cs, stepping, call stack,
      locals, watch and hover, Console output. Still open: the WebAssembly part of a Blazor Web App.
- [x] Git pull that isn't fast-forward only: diverged branches are merged, or rebased with `pull.rebase` (or
      `branch.<name>.rebase`); `pull.ff = only` is honored. A pull that stops at conflicts says what to do next;
      mid-rebase, Commit (or Git: Continue Rebase) goes on once the files are resolved, Git: Abort Rebase goes back.

## 6. The rest of a .NET project

- [ ] Highlighting beyond C#, Razor, JSON, XML and YAML: JavaScript / TypeScript, CSS / SCSS, HTML, SQL, Markdown,
      PowerShell and shell scripts.
- [ ] Linked editing: renaming an XML or HTML tag renames its closing tag (`.csproj`, `.razor`, `.cshtml`).
- [ ] Tasks, built in (not a module): a Tasks view and "Tasks: Run Task…" (quick pick; Rerun Last Task) that run
      each task in a terminal. ByteForge's own tasks live in `.byteforge/tasks.json` in the workspace root (the
      first file of a per-folder `.byteforge/` folder; no `.vscode/tasks.json`): `name`, `command`, `cwd`
      (relative to the root), `group` (a header in the view), `saveAll` (default true), comments allowed;
      "Tasks: Configure Tasks" creates it with an example. Next to them, found in the workspace: package.json
      scripts (npm, pnpm, yarn or bun, from the lock file), Makefile targets and justfile recipes.
- [ ] EF Core view, built in: a side bar view with each project's `DbContext`s and their migrations, marked
      applied or pending once EF has asked the database (shown at once from the source, as "Update Database to
      Migration…" does). Click a migration: its file. Right-click: Update Database to Here, Script from Here,
      Remove (the last one). Top row: Add Migration, Update Database, Drop Database, Scaffold DbContext, Refresh.
      Uses the Run menu's EF commands; refreshes after each one and when a Migrations folder changes.

## 7. Later

- [ ] Code coverage in the gutter.
- [ ] Sticky scroll (the current type and member pinned at the top of the editor).
- [ ] An AI assistant panel.
- [ ] A keyboard shortcuts editor (today: `"keybindings"` in settings.json).
- [ ] Vim mode.
- [ ] Remote development: SSH, WSL, dev containers.
- [ ] Profiling: `dotnet-counters` / `dotnet-trace` from the Run menu.
