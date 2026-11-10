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

## 2. Editor

- [x] Code folding (braces, `#region`; indentation for XML/YAML), with gutter chevrons. Still open: folding `using` blocks and comment blocks.
- [x] Multiple cursors: Alt+Click, Ctrl+Alt+Up/Down, Ctrl+D to add the next match (plus Ctrl+Shift+L). Line commands (comment, move, duplicate) and completions still work on one cursor.
- [x] Split editor (two editor groups side by side): Ctrl+\\, Ctrl+1/2. Still open: the same file in both groups (one shared buffer) and dragging tabs between groups.
- [x] User snippets (`prop`, `ctor`, `foreach`, `try`, …) offered in completion, with Tab between fields.
- [x] Minimap.

## 3. .NET workflow

- [x] Test Explorer: run/debug one test, a class or all; pass/fail markers. Tests are found in the source (test attributes), not with `dotnet test --list-tests`: instant, and it knows where each test is. Still open: tests generated at runtime (e.g. `[MemberData]` cases listed individually) and live per-test progress during long runs.
- [x] Solution view: projects from the `.sln`/`.slnx` in their solution folders (or every `.csproj` without a solution), with target framework and kind, NuGet packages (central versions from `Directory.Packages.props` included) and project references; build, run, test, reveal and open from each project.
- [x] NuGet: search the feeds as you type ("NuGet: Add Package…" or + on a project's Packages), update and remove from the Solution view, and "Check for Package Updates" marks newer versions (`dotnet package search`, `dotnet add/remove package`, `dotnet list package --outdated`). Still open: prerelease versions and picking a version other than the latest.
- [ ] Hot reload: "Run with Watch" (`dotnet watch`).
- [ ] Launch profile picker for F5 / Ctrl+F5 (`launchSettings.json`).
- [ ] New Project / New Item from `dotnet new` templates.
- [ ] User secrets editor (`dotnet user-secrets`).
- [ ] EF Core migrations commands (`dotnet ef migrations add/remove`, `database update`).

## 4. Nice to have

- [ ] Git blame and file history; merge-conflict actions (accept current/incoming/both).
- [ ] Debugging inside containers (netcoredbg in the container, attach through `docker exec`).

## Done

- [x] Module packages: a module registry (`modules/` on the `packages` branch) with install/update/disable/uninstall in the Modules view; CSharpier preinstalled; TODO Finder, JSON Formatter, GUID Generator, .NET Project Files and Focus Timer packages.

- [x] Dockerfile and Compose (YAML) highlighting, Docker view, Add Docker Support / Compose File.
