# Modules

Modules add commands, formatters, status bar items, side bar views, file icons and languages. They're packages:
the **Modules** view (puzzle icon) lists what's **installed** and what's **available** from the ByteForge module
registry. Search filters both lists; each module has a tile in one of the current theme's syntax colors. The
switch turns an installed module on or off, **Install** and **Update** do what they say, and clicking a module
opens its details (full description, version, requirements, the file types it formats, Show files and Uninstall).

| Module | What it does |
|---|---|
| **CSharpier** | C# / MSBuild formatter, **preinstalled** (copied into your modules folder on first start; uninstall it like any other) |
| **Docker** | the Docker view, Docker commands, Dockerfile highlighting and Docker file icons, **preinstalled** |
| TODO Finder | TODO / FIXME / HACK comments: count in the status bar, "TODOs: List All" with clickable locations |
| JSON Formatter | Format Document for `.json` files; leaves JSON with comments alone |
| GUID Generator | "GUID: Insert New GUID" (Ctrl+Alt+G), uppercase, `{braced}` and `new Guid("…")` variants |
| .NET Project Files | "Files: Add .gitignore / .editorconfig / Directory.Build.props / global.json / .gitattributes" |
| Focus Timer | a Pomodoro countdown in the status bar, with desktop notifications |

The registry is the `modules/` folder of the `packages` branch (`modules/index.json` lists the packages and
their files; a workflow regenerates it). `"modules.registry"` in settings points ByteForge at another one (also
`file://`). Downloads use `curl`; installed modules live in `~/.config/byteforge/modules/<name>/`.

The modules in `<install dir>/modules/` (`modules/csharpier`, `modules/docker`) are preinstalled: they're copied
into the user's modules folder once, and an uninstalled one isn't copied again. The CSharpier module uses a
version pinned in the project's dotnet tool manifest (`dotnet tool install csharpier` in the repository), else a
global one. Without either, the status bar offers to run `dotnet tool install -g csharpier`. It needs `python3`.

## Writing a module

A module is a folder in `~/.config/byteforge/modules/` with a `module.json`:

```json
{ "id": "me.hello", "name": "Hello", "version": "1.0.0", "author": "your-github-name", "description": "…", "command": "python3", "args": ["hello.py"] }
```

On the module's page the author links to `https://github.com/<author>`; set `"authorUrl"` to link somewhere else.
To publish it, add the folder to `modules/` on the `packages` branch.

`module.json` can also declare, without any code:

- **`fileIcons`**: SVG path data (24×24 view box, even-odd fill) for the files it's for, matched ignoring case by
  exact `names`, `patterns` with `*`, or `extensions`. `color` is a theme color key (`icon.docker`) or a color
  (`#2496ed`). Module icons win over the built-in ones.
- **`languages`**: an `id`, `name`, the files (`names`, `patterns`, `extensions`), a `lineComment` and highlighting
  `rules`: `{ "match": "<regex>", "token": "keyword", "group": 1 }`. Rules apply per line; where matches overlap, the
  earlier rule wins. `token` is a theme token name (`keyword`, `string`, `comment`, `number`, `class name`, …).
- **`views`**: side bar views (`id`, `title`, `icon`: a built-in icon name or path data). Each gets an activity bar
  item; the module fills it with `setTree`.

A module that only declares things leaves out `"command"` and isn't started. Everything a module adds shows while
it's installed and turned on. `modules/docker` uses all of it.

## Protocol

ByteForge starts the command in the module's folder and exchanges one JSON object per line.

**Module → IDE**

- `{"type":"registerCommand","id","title","category","keybinding"}`
- `{"type":"registerFormatter","id","name","extensions":["cs","csproj"]}`: become a formatter for these files
- `{"type":"formatResult","requestId","text"}` or `{…,"error"}`: the answer to a `format` request
- `{"type":"status","id","text","tooltip","command","alignment":"left|right"}`
- `{"type":"log","text"}`
- `{"type":"openFile","path","line","column"}`
- `{"type":"showView","id"}`
- `{"type":"insertText","text"}`: insert at the caret(s) of the active editor
- `{"type":"setTheme","name"}`: switch to a color theme (for example one the module wrote to `~/.config/byteforge/themes/`)
- `{"type":"setTree","view","items","message","error","badge","actions"}`: the content of one of its views. Items:
  `{"id","label","description","tooltip","icon","iconColor","header","count","expanded","children","actions"}`;
  `icon` is a built-in icon name, path data or `file:<name>` (that file's icon); `header` items are section headers;
  `actions` (`{"id","icon","tooltip"}`) are buttons on a hovered row, or in the view's top row. `badge` is a number
  on the activity bar item.
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
