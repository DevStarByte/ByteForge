# ByteForge roadmap: an IDE you can work in every day

Worked through top to bottom. Tick items off as they land. What the first roadmap built is in the README and
the git history.

## 1. Make what's there trustworthy

- [x] Dogfooding: use ByteForge for real work and fix what turns up. Every feature was clicked through headlessly
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
      in the terminal; Problems lists build errors of closed C# files while a language server runs; Linux packages
      install the app icon (1.1.1); a diff keeps showing unsaved edits through a git status refresh (1.1.2); on
      Windows, updating an install for all users asks for admin rights instead of failing with "access denied"
      (1.1.3); after a restore the C# server restarts so restored packages resolve, and the Solution view lists
      solution items (1.1.4, from a dogfooding session on a real Aspire + Blazor solution: open, edit with
      completion, restore, build, run, debug, Debug Console, commit). Done as a roadmap item: from here on, bugs
      found in daily use are fixed as they come.
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

- [x] Highlighting beyond C#, Razor, JSON, XML and YAML: JavaScript / TypeScript, CSS / SCSS, HTML, SQL, Markdown,
      PowerShell and shell scripts.
- [x] Linked editing: renaming an XML or HTML tag renames its closing tag (`.csproj`, `.razor`, `.cshtml`).
- [x] Tasks, built in (not a module): a Tasks view and "Tasks: Run Task…" (quick pick; Rerun Last Task) that run
      each task in a terminal. ByteForge's own tasks live in `.byteforge/tasks.json` in the workspace root (the
      first file of a per-folder `.byteforge/` folder; no `.vscode/tasks.json`): `name`, `command`, `cwd`
      (relative to the root), `group` (a header in the view), `saveAll` (default true), comments allowed;
      "Tasks: Configure Tasks" creates it with an example. Next to them, found in the workspace: package.json
      scripts (npm, pnpm, yarn or bun, from the lock file), Makefile targets and justfile recipes.
- [x] EF Core view, built in: a side bar view with each project's `DbContext`s and their migrations, marked
      applied or pending once EF has asked the database (shown at once from the source, as "Update Database to
      Migration…" does). Click a migration: its file. Right-click: Update Database to Here, Script from Here,
      Remove (the last one). Top row: Add Migration, Update Database, Drop Database, Scaffold DbContext, Refresh.
      Uses the Run menu's EF commands; refreshes after each one and when a Migrations folder changes.
- [x] Benchmarks, built in: BenchmarkDotNet classes and their `[Benchmark]` methods found in the source, with
      Run Benchmark (Code Lens and right-click) building in Release and running just that one in a terminal
      (`--filter`). Results from `BenchmarkDotNet.Artifacts/results/*-report-full.json` in a Benchmarks panel
      tab: mean, error, allocated per method, and the change since the previous run (green faster, red slower).
- [x] Live metrics, built in: the running .NET processes (the app being run or debugged first) and, for the
      chosen one, CPU, working set, GC heap, GC counts, thread pool queue, exceptions and ASP.NET Core requests
      per second as sparklines in a panel tab (`dotnet-counters collect --format json`, its file read as it
      grows; offers to install the tool). The status bar shows CPU and memory of the app while it runs. A first step
      toward the profiling item under Later.
- [x] Resource editor, built in: `.resx` files open in a table with one row per key and one column per language
      (`Strings.resx`, `Strings.de.resx`, …): edit values in place, add and remove keys in every language at
      once, missing translations marked and filterable, comments as a column. Saving writes each `.resx` with
      only the changed `<data>` entries touched. Problems lists keys missing in a language.
- [x] Central Package Management, built in: in the Solution view's NuGet, each package's version from
      `Directory.Packages.props` and the projects that use it; `PackageReference`s that still carry their own
      `Version` (or `VersionOverride`) are listed with "Move to Directory.Packages.props". "Enable Central Package
      Management" writes the props file from the projects' current versions and removes them from the projects.

## 7. Windows Forms

- [x] A Windows Forms designer, built in: View Designer (Shift+F7, the tab strip's button on a form) opens a
      form or user control from its `.Designer.cs`, drawn by ByteForge (Windows 11 look; works on Linux and
      macOS). Toolbox (common controls, containers, menus, data, components) by click, double-click or drag;
      select, move, resize (handles, arrow keys, Shift+arrows), drop into containers, Bring to Front / Send to
      Back, Delete; a property grid by category with switches, choices, colors, anchors, fonts, list items and
      every other property as code; events (double-click: `<name>_<Event>` added to the code file and opened);
      a component tray. Changes are written into `InitializeComponent` through the file's editor (undo, save,
      autosave and git as for any file) in the file's style, statements it doesn't know kept as written; checked
      with `dotnet build` on .NET 11 (both styles).
- [x] Copy, cut and paste of controls (Ctrl+C / X / V, with what is inside them, within a form or into another
      one, in that file's style), Select All; snap lines while dragging and resizing (edges, the recommended
      spacing, the container's edge; Alt: off); Align Lefts / Centers / Rights / Tops / Middles / Bottoms and
      Same Size on several selected controls.
- [x] Menu, tool and status strip items and grid columns, edited on the form: a selected menu item opens its menu
      (and submenus), "Type Here" adds an item whose typed text names it (`&File` → `fileToolStripMenuItem`),
      separators, Move Up / Down (Left / Right) and Delete; a grid's column headers at their widths, "+" and the
      toolbar's Add Column (text, check box, combo box, button, link, image). Items and columns have their own
      properties in the grid (Checked, ShortcutKeys, HeaderText, Width, …).
- [x] Localized forms (`resources.ApplyResources`): the designer shows a control's values from `Form1.resx`,
      or from a language's file over it (Language: (Default) / de / … in the toolbar); moving, resizing and
      typing a property the resources hold write it to the shown language's file (typed as in the file; a
      language's file is made when it has none), saved with the designer and undone with it. Copying a
      localized control takes its values along as code; deleting one removes its entries from every language.

## 8. Later

- [x] Modules in C# instead of Python: one .NET module host runs them (each in its own load context), the module
      SDK lives with the modules on the packages branch, every module is ported, and ByteForge no longer runs
      Python modules (installed ones are replaced by their .NET versions at start).
- [x] Code coverage in the gutter: Run All Tests (or the test at the cursor) with Coverage, from the Test menu or
      the Testing view, collects coverage (coverlet when the test project references it, else Microsoft's collector,
      which comes with Microsoft.NET.Test.Sdk) and shows covered, partly covered (a branch not taken) and uncovered
      lines as bars in the gutter; the Output sums it up (lines and branches, without the test projects) and lists
      the least covered files. Show or Hide Coverage, Clear Coverage; edited lines lose their bar.
- [x] Sticky scroll: the first lines of the blocks the top of the view is in (from the fold regions, without
      comment runs and `using` blocks) pinned at the top, pushed out by their block's end; click: go there.
      `"editor.stickyScroll.enabled"`, `"editor.stickyScroll.maxLineCount"` (5).
- [ ] An AI assistant panel.
- [x] A keyboard shortcuts editor: Preferences → Keyboard Shortcuts lists every command with its keys (search by
      name or keys); click the keys and press new ones (two presses for a chord), Remove, Reset; keys another
      command has too are pointed out. Writes `"keybindings"` in settings.json; changes take effect at once.
- [ ] Vim mode.
- [ ] Remote development: SSH, WSL, dev containers.
- [ ] Profiling: `dotnet-counters` / `dotnet-trace` from the Run menu.
