# Modules

Modules add commands, formatters, status bar items, side bar views, file icons and languages. They're packages:
the **Modules** view (puzzle icon) lists what's **installed** and what's **available** from the ByteForge module
registry. Search filters both lists; each module has a tile in one of the current theme's syntax colors. The
switch turns an installed module on or off, **Install** and **Update** do what they say, and clicking a module
opens its details (full description, version, requirements, the file types it formats, Show files and Uninstall).

| Module | What it does |
|---|---|
| **ReSharper Formatter** | formats C# with JetBrains' ReSharper command line tools (`jb cleanupcode`, the engine of Rider and ReSharper), following `.editorconfig` and the solution's `.DotSettings`; **preinstalled** (copied into your modules folder on first start; uninstall it like any other) |
| **CSharpier** | C# / MSBuild formatter (a .NET module); install it from the Modules view |
| **Docker** | the Docker view, Docker commands, Dockerfile highlighting and Docker file icons; install it from the Modules view |
| **Aspire** | the Aspire view: AppHosts and their running resources (state, health, endpoints, start / stop / restart, logs), the dashboard, and run / new / init / add / publish / deploy commands; needs the [Aspire CLI](https://aspire.dev); install it from the Modules view |
| **SQL** | the Database view: connections from `"sql.connections"` in settings, appsettings.json `ConnectionStrings` and SQLite files, with their tables, views and columns; "SQL: Run File" (Ctrl+Shift+Enter), Select Top 100, a shell per connection, SQL highlighting and a keyword-case formatter. SQLite needs only Python; PostgreSQL, MySQL / MariaDB and SQL Server use `psql`, `mysql` and `sqlcmd`; install it from the Modules view |
| TODO Finder | TODO / FIXME / HACK comments: count in the status bar, "TODOs: List All" with clickable locations |
| JSON Formatter | Format Document for `.json` files; leaves JSON with comments alone |
| GUID Generator | "GUID: Insert New GUID" (Ctrl+Alt+G), uppercase, `{braced}` and `new Guid("…")` variants |
| .NET Project Files | "Files: Add .gitignore / .editorconfig / Directory.Build.props / global.json / .gitattributes" |
| Focus Timer | a Pomodoro countdown in the status bar, with desktop notifications |

The registry is the `modules/` folder of the `packages` branch (`modules/index.json` lists the packages and
their files; a workflow regenerates it). `"modules.registry"` in settings points ByteForge at another one (also
`file://`). Downloads use `curl`; installed modules live in `~/.config/byteforge/modules/<name>/`.

Only the ReSharper Formatter is preinstalled: `<install dir>/modules/resharper-format` is copied into the user's
modules folder once, and isn't copied again after it's uninstalled. Every other module comes from the registry
(CSharpier, preinstalled before 0.10.0, stays installed for those who have it). The ReSharper module uses a version
of the tools pinned in the project's dotnet tool manifest (`dotnet tool install JetBrains.ReSharper.GlobalTools` in
the repository), else a global one; "ReSharper: Install" installs them globally. `"resharper.profile"` picks the
cleanup profile (default `Built-in: Reformat Code`). It needs `python3` and the .NET SDK.

The SQL module's settings: `"sql.connections"` (a list of `{ "name", "driver", "connectionString" }`, or
`host`, `port`, `database`, `user`, `password`, `file` for SQLite, `integratedSecurity` and
`trustServerCertificate` instead of a connection string; drivers: `sqlite`, `postgres`, `mysql`, `sqlserver`),
`"sql.discover": false` to skip appsettings and SQLite files, `"sql.format.keywordCase"` (`upper`, `lower`,
`preserve`) and `"sql.postgresPath"` / `"sql.mysqlPath"` / `"sql.sqlserverPath"` for clients that aren't on PATH.
Queries and shells run in a terminal; passwords go to the client through its environment, never the command line.

## Writing a module

A module is a folder in `~/.config/byteforge/modules/` with a `module.json`. Modules are written in C# and run
in ByteForge's .NET module host (they need .NET 10 or newer, which a C# developer has anyway):

```json
{ "id": "me.hello", "name": "Hello", "version": "1.0.0", "author": "your-github-name", "description": "…", "runtime": "dotnet", "assembly": "bin/Hello.dll" }
```

One host process runs every .NET module, each in its own load context: turning a module off, updating or
uninstalling it unloads it without touching the others, and a module that throws stops alone (its error goes to
the Output). The host reads a module's files into memory, so none stays open. It's `ByteForge.ModuleHost.dll` in
`modulehost/` next to ByteForge (`dotnet build modulehost` when running from the source tree).

A module is a class library (`net10.0`) built against the module SDK, `sdk/ByteForge.Modules` on the
[`packages` branch](https://github.com/DevStarByte/ByteForge/tree/packages), and carries the SDK's DLL in its
`bin/`. It derives from `Module`, overrides what it reacts to and has the entry point the host looks for:

```csharp
using ByteForge.Modules;

public static class Entry
{
    public static Task ByteForgeMain(TextReader input, TextWriter output, string moduleDir, CancellationToken cancel) =>
        ModuleRunner.RunAsync<HelloModule>(input, output, moduleDir, cancel);
}

public sealed class HelloModule : Module
{
    protected override Task OnStartAsync()
    {
        RegisterCommand("hello.say", "Say Hello", "Hello");
        return Task.CompletedTask;
    }

    protected override Task OnCommandAsync(string id)
    {
        Log($"Hello from {Workspace ?? "nowhere"}");
        return Task.CompletedTask;
    }
}
```

`Module` has a method for each message below (`Log`, `SetStatus`, `RegisterFormatter` with `FormatAsync`,
`SetTree`, `QuickPickAsync`, `PromptAsync`, …), the user's `Settings`, `Workspace`, `ModuleDir`, `ConfigDir` and
`Stopping` (cancelled when the module is unloaded: background loops end on it). `Programs.Which` finds a program
on PATH (never in the current folder) and `Programs.RunAsync` runs one. `ModuleHarness`
(`sdk/ByteForge.Modules.Testing`) runs a module in memory for its tests; [CSharpier](https://github.com/DevStarByte/ByteForge/tree/packages/modules/csharpier)
is a small complete example with tests. On the packages branch a workflow builds each module into its `bin/`,
runs the tests and lists `module.json` and `bin/` in the registry.

A module with a `"command"` instead (`"command": "python3", "args": ["hello.py"]`) is started as a process of
its own and speaks the same protocol over stdin and stdout; the modules that are still in Python work that way.

**Themes:** a module that makes a color theme writes it into its own `themes/` folder
(`Path.Combine(ModuleDir, "themes", "mine.json")`). ByteForge lists those themes while the module is installed
and turned on, reloads them when they change, and drops them with the module; if the theme in use goes, it
switches to ByteForge Dark (or Light). `SetTheme` switches to one.

On the module's page the author links to `https://github.com/<author>`; set `"authorUrl"` to link somewhere else.
To publish it, add the folder to `modules/` on the `packages` branch.

`module.json` can also declare, without any code:

- **`fileIcons`**: SVG path data (24×24 view box, even-odd fill) for the files it's for, matched ignoring case by
  exact `names`, `patterns` with `*`, or `extensions`. `color` is a theme color key (`icon.docker`) or a color
  (`#2496ed`). Module icons win over the built-in ones.
- **`languages`**: an `id`, `name`, the files (`names`, `patterns`, `extensions`), a `lineComment` and highlighting
  `rules`: `{ "match": "<regex>", "token": "keyword", "group": 1 }`. Rules apply per line; where matches overlap, the
  earlier rule wins. `token` is a theme token name (`keyword`, `string`, `comment`, `number`, `class name`, …).
  `detect` lists regexes that recognize the language in an untitled editor, which has no file name to go by
  (`^` and `$` match at every line; add `(?i)` to ignore case). Each pattern that matches the start of the text
  counts once: two or more win over the built-in guesses (C#, JSON, XML, Razor, YAML, INI, .env), a single one
  counts when nothing else fits. The [SQL](https://github.com/DevStarByte/ByteForge/tree/packages/modules/sql) and
  [Azure](https://github.com/DevStarByte/ByteForge/tree/packages/modules/azure) modules have examples.
- **`views`**: side bar views (`id`, `title`, `icon`: a built-in icon name or path data). Each gets an activity bar
  item; the module fills it with `setTree`.
- **`settings`**: the settings the module reads, listed on the settings screen (Ctrl+,) under **Extensions**, one
  group per module: `{ "key", "title", "description", "type", "default" }` with `type` `boolean`, `number`,
  `string`, `list` (of strings, edited comma-separated) or `json` (edited in settings.json); a `string` with
  `"choices": [...]` is picked from them. Values live in settings.json like any other setting and reach the module
  with `activate` and `settingsChanged`. A key ByteForge already has isn't listed again. The [SQL module](https://github.com/DevStarByte/ByteForge/tree/packages/modules/sql) has examples.

A module that only declares things leaves out `"command"` and isn't started. Everything a module adds shows while
it's installed and turned on. The [Docker module](https://github.com/DevStarByte/ByteForge/tree/packages/modules/docker) uses all of it.

## Protocol

One JSON object per line. A .NET module reads them from `input` and writes its own to `output`; a module with a
`"command"` is started in its own folder and uses stdin and stdout. (Between ByteForge and the .NET host each line
is `<module id>	<message>`, which the host strips; see `modulehost/Program.cs`.)

**Module → IDE**

- `{"type":"registerCommand","id","title","category","keybinding"}`
- `{"type":"registerFormatter","id","name","extensions":["cs","csproj"]}`: become a formatter for these files
- `{"type":"formatResult","requestId","text"}` or `{…,"error"}`: the answer to a `format` request
- `{"type":"status","id","text","tooltip","command","alignment":"left|right"}`
- `{"type":"log","text"}`
- `{"type":"openFile","path","line","column"}`: `line` and `column` count from 0
- `{"type":"showView","id"}`
- `{"type":"insertText","text"}`: insert at the caret(s) of the active editor
- `{"type":"setTheme","name"}`: switch to a color theme (for example one the module wrote into its own `themes/` folder)
- `{"type":"setTree","view","items","message","error","badge","actions"}`: the content of one of its views. Items:
  `{"id","label","description","tooltip","icon","iconColor","header","count","expanded","children","actions","menu"}`;
  `icon` is a built-in icon name, path data or `file:<name>` (that file's icon); `header` items are section headers;
  `actions` (`{"id","icon","tooltip","label"}`) are buttons on a hovered row, or in the view's top row. A row's
  right-click menu lists its `actions` (by `label`, else `tooltip`), then its `menu` entries (same shape, no icon
  needed; `{"id":"-"}` is a separator); choosing one sends the same `treeAction`. The menu is drawn by the IDE, like
  the Explorer's, so every module looks the same. `badge` is a number on the activity bar item.
- `{"type":"runInTerminal","title","command","cwd","saveAll"}`: run a command line in a new terminal tab
- `{"type":"quickPick","requestId","placeholder","items":[{"label","detail","hint","value"}]}`: a pick list
- `{"type":"prompt","requestId","message","detail","buttons":["Remove","Cancel"]}`: a question

**IDE → module**

- `{"type":"activate","workspace","settings"}` (`settings`: the user's settings object)
- `{"type":"treeAction","view","item","action"}`: a row (`"click"`) or button was clicked (`item` is null for the top row)
- `{"type":"quickPickResult","requestId","value"}` (only when something was picked), `{"type":"promptResult","requestId","index"}` (null when dismissed)
- `{"type":"executeCommand","id"}`
- `{"type":"format","requestId","path","text"}` (formatter modules only)
- `{"type":"event","name":"folderOpened|fileSaved|themeChanged|editorChanged","path"}` (`editorChanged`: the active tab changed; `path` is null for untitled documents and pages)
- `{"type":"event","name":"viewShown|viewHidden","view"}`, `{"type":"event","name":"filesChanged"}`, `{"type":"event","name":"settingsChanged","settings"}`
