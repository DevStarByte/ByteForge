# ByteForge roadmap: a complete C# IDE

What ByteForge still needs to be a good everyday C# IDE, in priority order. Tick items off as they land.

## 1. Language intelligence (csharp-ls features not used yet)

The language server already supports all of these; the client needs the requests, and the editor needs the UI.

- [x] **Find All References** (Shift+F12): a References panel listing each use with its line of code, grouped by file. Click to jump.
- [x] **Rename Symbol** (F2): prompt with the current name, apply the workspace edit across files (open and closed).
- [x] **Signature Help**: parameter hints while typing a call (on `(` and `,`, closes on `)` and Escape), with the active parameter in bold.
- [x] **Go to Implementation** (Ctrl+F12) and **Go to Type Definition**: one result jumps, several go to the References panel.
- [x] **Document Symbols**: Go to Symbol in Editor in the palette ("@"). Still open: a key (Ctrl+Shift+O opens a folder today) and symbol breadcrumbs.
- [x] **Workspace Symbols** (Ctrl+T): find any type or member by name ("#").
- [x] **Document Highlight**: highlight the other uses of the symbol under the caret.
- [x] **Inlay Hints**: inferred `var` types and parameter names (setting `editor.inlayHints`).
- [x] **Refactoring**: Refactor… (Ctrl+Shift+R) lists only refactorings, grouped (Rename, Extract, Introduce, Inline, Move, Generate, Rewrite), plus a Refactor menu, an editor right-click menu and one command per refactoring with Visual Studio's chords (Extract Method Ctrl+R Ctrl+M, Extract Interface Ctrl+R Ctrl+I, Remove and Sort Usings Ctrl+R Ctrl+G, Rename Ctrl+R Ctrl+R). A single match applies at once; the name a refactoring makes up opens Rename (`refactor.renameGenerated`). Edits that write to a new file (Extract Interface) create it. Still open: a preview of multi-file changes before applying, and Change Signature (Roslyn's needs a dialog csharp-ls doesn't have).

## 2. Editor

- [x] Code folding (braces, `#region`; indentation for XML/YAML), with gutter chevrons. Still open: folding `using` blocks and comment blocks.
- [x] Multiple cursors: Alt+Click, Ctrl+Alt+Up/Down, Ctrl+D to add the next match (plus Ctrl+Shift+L). Line commands (comment, move, duplicate) and completions still work on one cursor.
- [x] Split editor (two editor groups side by side): Ctrl+\\, Ctrl+1/2. Still open: the same file in both groups (one shared buffer) and dragging tabs between groups.
- [x] User snippets (`prop`, `ctor`, `foreach`, `try`, …) offered in completion, with Tab between fields.
- [x] Minimap.

## 3. .NET workflow

- [x] Test Explorer: run/debug one test, a class or all; pass/fail markers. Tests are found in the source (test attributes), not with `dotnet test --list-tests`: instant, and it knows where each test is. Still open: tests generated at runtime (e.g. `[MemberData]` cases listed individually) and live per-test progress during long runs.
- [x] Solution view: projects from the `.sln`/`.slnx` in their solution folders (or every `.csproj` without a solution), with target framework and kind, NuGet packages (central versions from `Directory.Packages.props` included) and project references under Dependencies, and each project's folders and files like Visual Studio's Solution Explorer (no `bin`/`obj`, nested `.cshtml.cs`/`appsettings.*.json`); build, run, test, reveal and open from each project.
- [x] NuGet: search the feeds as you type ("NuGet: Add Package…" or + on a project's Packages), update and remove from the Solution view, and "Check for Package Updates" marks newer versions (`dotnet package search`, `dotnet add/remove package`, `dotnet list package --outdated`). Still open: prerelease versions and picking a version other than the latest.
- [x] Hot reload: "Run with Watch" (Alt+F5, or the flame on an app in the Solution view) runs `dotnet watch run`; edits apply while the app runs, rude edits restart it by themselves, and compiler errors from each attempt go to Problems. Still open: a manual "restart now" (Ctrl+R in a terminal).
- [x] Launch profile picker: "Select Launch Profile…" lists the startup project's `launchSettings.json` profiles; the pick is remembered per project and used by F5, Ctrl+F5 and Run with Watch (`--launch-profile`). The current profile shows in the status bar; click it to switch.
- [x] New Project / New Item: New Project lists the `dotnet new` project templates, creates the project in its own folder and adds it to the solution; New Item (File menu, palette, or right-click a folder in the Explorer) offers C# class, interface, record, enum and struct with the namespace from the project and folder, plus the SDK's item templates (controllers, Razor pages, `.gitignore`, `Directory.Build.props`, …).
- [x] User secrets: "Manage User Secrets" opens `secrets.json` of the current file's project (or the startup project), running `dotnet user-secrets init` first when the project has none. Still open: a form view instead of the JSON.
- [ ] EF Core migrations commands (`dotnet ef migrations add/remove`, `database update`).

## 4. Nice to have

- [x] Diff editor: side by side (rows aligned, scrolling together) or inline, syntax highlighted, the files' line numbers, changed words marked inside changed lines, +/− counts, Next/Previous Change (F7 / Shift+F7) and Open File at the change. Diff colors fall back to defaults in themes without `diffEditor.*` keys. Still open: staging or reverting single hunks from the diff, collapsing unchanged regions, and editing the working-tree side.
- [ ] Git blame and file history; merge-conflict actions (accept current/incoming/both).
- [ ] Debugging inside containers (netcoredbg in the container, attach through `docker exec`).

## Done

- [x] Module packages: a module registry (`modules/` on the `packages` branch) with install/update/disable/uninstall in the Modules view; CSharpier preinstalled; TODO Finder, JSON Formatter, GUID Generator, .NET Project Files and Focus Timer packages.

- [x] Dockerfile and Compose (YAML) highlighting, Docker view, Add Docker Support / Compose File.
