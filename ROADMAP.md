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
- [ ] Multiple cursors: Alt+Click, Ctrl+Alt+Up/Down, Ctrl+D to add the next match.
- [ ] Split editor (two editor groups side by side).
- [x] User snippets (`prop`, `ctor`, `foreach`, `try`, …) offered in completion, with Tab between fields.
- [ ] Minimap.

## 3. .NET workflow

- [ ] Test Explorer: discover tests (`dotnet test --list-tests`), run/debug one test, a class or all; pass/fail markers.
- [ ] Solution view: projects from the `.sln`/`.slnx`, their dependencies and project references.
- [ ] NuGet: search, add, update and remove packages (`dotnet add package`, `dotnet list package --outdated`).
- [ ] Hot reload: "Run with Watch" (`dotnet watch`).
- [ ] Launch profile picker for F5 / Ctrl+F5 (`launchSettings.json`).
- [ ] New Project / New Item from `dotnet new` templates.
- [ ] User secrets editor (`dotnet user-secrets`).
- [ ] EF Core migrations commands (`dotnet ef migrations add/remove`, `database update`).

## 4. Nice to have

- [ ] Git blame and file history; merge-conflict actions (accept current/incoming/both).
- [ ] Debugging inside containers (netcoredbg in the container, attach through `docker exec`).

## Done

- [x] Dockerfile and Compose (YAML) highlighting, Docker view, Add Docker Support / Compose File.
