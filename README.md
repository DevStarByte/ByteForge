<div align="center">

# ByteForge

**A fast, modular, VS Code–style IDE for C#, built from scratch in Rust.**

GPU-rendered with [GPUI](https://github.com/zed-industries/zed) (the UI framework behind Zed) · themeable down to the last pixel · extensible in any language

[![Version](https://img.shields.io/github/v/tag/DevStarByte/ByteForge?label=version&color=blue&sort=semver)](https://github.com/DevStarByte/ByteForge/tags)
[![Last commit](https://img.shields.io/github/last-commit/DevStarByte/ByteForge)](https://github.com/DevStarByte/ByteForge/commits/master)
[![Stars](https://img.shields.io/github/stars/DevStarByte/ByteForge?style=flat&logo=github)](https://github.com/DevStarByte/ByteForge/stargazers)
[![Issues](https://img.shields.io/github/issues/DevStarByte/ByteForge)](https://github.com/DevStarByte/ByteForge/issues)
[![License: MIT](https://img.shields.io/badge/license-MIT-green)](Cargo.toml)

[![Rust][rust-badge]](https://www.rust-lang.org/)
[![GPUI](https://img.shields.io/badge/UI-GPUI-084CCF?logo=zedindustries&logoColor=white)](https://github.com/zed-industries/zed)
[![C#](https://img.shields.io/badge/C%23-first--class-239120?logo=dotnet&logoColor=white)](#languages)
[![.NET](https://img.shields.io/badge/.NET-SDK-512BD4?logo=dotnet)](https://dotnet.microsoft.com/)
[![Razor](https://img.shields.io/badge/Razor%20%2F%20Blazor-supported-5C2D91?logo=blazor&logoColor=white)](#languages)
[![LSP](https://img.shields.io/badge/LSP-csharp--ls-informational)](https://github.com/razzmatazz/csharp-language-server)
[![DAP](https://img.shields.io/badge/DAP-netcoredbg-informational)](https://github.com/Samsung/netcoredbg)
[![Docker](https://img.shields.io/badge/Docker-module-2496ED?logo=docker&logoColor=white)](#docker)
[![tree-sitter](https://img.shields.io/badge/highlighting-tree--sitter-green)](https://tree-sitter.github.io/)

[![Linux](https://img.shields.io/badge/Linux-X11%20%7C%20Wayland-FCC624?logo=linux&logoColor=black)](#getting-started)
[![Windows](https://img.shields.io/badge/Windows-supported-0078D6?logo=windows&logoColor=white)](#getting-started)
[![Arch](https://img.shields.io/badge/Arch-pacman%20repo-1793D1?logo=archlinux&logoColor=white)](#arch-linux)
[![Vulkan](https://img.shields.io/badge/renderer-Vulkan-AC162C?logo=vulkan&logoColor=white)](#arch-linux)

[Features](#features) · [Getting started](#getting-started) · [Themes](#themes) · [Layout](#layout) · [Settings](#settings-and-keybindings) · [Modules](#modules) · [Architecture](#architecture) · [Roadmap](#limitations-and-next-steps)

</div>

---

## Highlights

| Area | What you get |
|---|---|
| **Real C# intelligence** | Roslyn-based `csharp-ls`: completion, hover, go to definition, references, rename, quick fixes, inlay hints |
| **Debugger** | netcoredbg over DAP: breakpoints, stepping, variables, call stack, Debug Console, ASP.NET Core launch profiles |
| **Test Explorer** | xUnit, NUnit and MSTest: run or debug any test, gutter markers, failure details |
| **Git built in** | Source Control view, gutter change markers, diffs, branches, pull / push / fetch / sync |
| **Docker** | containers, images, volumes, networks, Compose, and one-click Dockerfile generation |
| **Themeable everything** | 127 colors, 99 metrics, fonts, icons and the whole window layout, hot-reloaded |
| **Modules** | out-of-process plugins in any language, over JSON on stdio; a broken one can't crash the IDE |
| **Fast** | rope buffer, viewport-only shaping, background tree-sitter highlighting, nothing blocks the UI |

Runs on **Linux** (X11 and Wayland, including Arch) and **Windows**.

---

## Features

### Editor

A custom editor on a rope-backed buffer (`ropey`) that renders only the visible lines and caches the shaped text
per line.

- **Editing:** undo/redo that groups typing, auto-indent, bracket/quote auto-close, comment toggling,
  move/duplicate/delete lines, find & replace, and word/line selection by double/triple click.
- **Multiple cursors:** Alt+Click adds (or removes) a cursor, Ctrl+Alt+Up/Down adds one on the line above/below,
  Ctrl+D selects the word and then adds the next occurrence, Ctrl+Shift+L selects all occurrences. Typing,
  deleting, Enter, Tab, the arrow keys, Home/End and copy/paste work at every cursor (pasting one line per
  cursor when the clipboard has as many lines); Escape goes back to one cursor.
- **Minimap** at the right edge: the file in miniature in its syntax colors, with the visible part marked.
  Click or drag it to scroll. `"editor.minimap": false` hides it.
- **Folding:** chevrons in the gutter fold `{ }` blocks and `#region`s (indentation in XML and YAML);
  Ctrl+Shift+[ / ] fold and unfold at the caret, plus Fold All / Unfold All. Arrow keys step over folds.
- **Snippets:** `prop`, `propfull`, `ctor`, `class`, `foreach`, `for`, `if`, `try`, `switch`, `svm`, `test` and more
  in C# completion; Tab / Shift+Tab move between the fields, repeated fields follow. Add your own in
  `~/.config/byteforge/snippets/<language>.json` (VS Code's format; a user snippet replaces a built-in one
  with the same prefix).
- **Look:** smooth scrolling, an animated gliding caret (line, block, outline or underline style; blink,
  smooth, phase or solid), rainbow brackets, matching-bracket highlight, indent guides, visible whitespace,
  rulers and diagnostic squiggles. The other uses of the symbol under the caret are highlighted (assignments
  stronger), and **inlay hints** show parameter names and inferred types (`"editor.inlayHints": false` turns
  them off).
- **Instant syntax colors** from **tree-sitter**, computed around the viewport on background threads.
  Semantic colors are layered on top as they arrive.

### Languages

- **C#** via the **csharp-ls** language server (Roslyn-based, LSP): semantic highlighting, diagnostics,
  completion, Markdown hover, go to definition (F12 / Ctrl+Click), **Find All References** (Shift+F12, in a
  References panel grouped by file), **Go to Implementations** (Ctrl+F12) and **Type Definition**, **Go to Symbol**
  in the file ("@" in Go to File) or the whole workspace (Ctrl+T, "#"), **Rename Symbol** (F2, across files),
  **parameter hints** while typing a call (Ctrl+Shift+Space; the arrows switch overloads) and **quick fixes**:
  Ctrl+. on an error or warning lists the fixes (add a `using`, generate a type, …) and refactorings; picking one
  applies it, also across files.
- **Razor** (`.razor` components, `.cshtml` pages and views): tree-sitter highlighting of directives, control
  flow, embedded C# and HTML markup, `@* *@` comment toggling, and Razor compile errors from `dotnet build` as
  squiggles and in Problems.
- **XML** highlighting for MSBuild and .NET files (`.csproj`, `.props`, `.targets`, `.slnx`, `.config`, `.resx`,
  `.xaml`, `.nuspec`, …), with MSBuild references (`$(Property)`, `@(Item)`, `%(Metadata)`) in their own color and
  `<!-- -->` comment toggling.
- **Formatting:** Format Document (Shift+Alt+F), optional format on save. **CSharpier** is the default
  formatter for C# and MSBuild/XML files. It's a preinstalled module you can uninstall (and install again) in the
  Modules view; without it, or with `"editor.defaultFormatter": "csharp-ls"`, csharp-ls (Roslyn) formats C#.
  "Format Document With…" switches.

### Run, debug and test

- **.NET CLI:** build, run, test, clean and restore, with output streamed into the Output panel and compiler
  errors collected in Problems.
- **Debugger** (netcoredbg over the Debug Adapter Protocol): F5 builds the startup project and runs it under the
  debugger. ASP.NET Core apps get the URLs and environment of their `launchSettings.json` profile, and the
  browser opens once the app listens. You can also attach to a running .NET process.
  - breakpoints in `.cs`, `.razor` and `.cshtml` files (click left of the line numbers or press F9). They follow
    your edits and are saved per folder.
  - step over/into/out, pause, restart, stop. The paused line is highlighted.
  - a Run and Debug view (`Ctrl+Shift+D`) with call stack, expandable variables and the breakpoint list
  - a Debug Console with program output and expression evaluation in the selected frame
- **Test Explorer** (the beaker in the activity bar): the tests of every test project (xUnit, NUnit, MSTest) as
  project → class → test, found in the source so they show up right away. Run or debug a test, a class, a
  project or everything (hover a row, or "Test: Run/Debug Test at Cursor"), rerun the failed ones. Results come
  from `dotnet test` (TRX): pass/fail/skip icons, durations, theory cases, and the failure message and stack in
  the tooltip. Test methods get a marker in the gutter (▷ not run, ✓ passed, ✗ failed); clicking it runs the
  test. Debugging starts the test host under the debugger, so breakpoints in tests work.
- **Clickable file references** in Output, the Debug Console (click) and the terminal (Ctrl+Click): MSBuild
  errors (`File.cs(12,5)`), .NET stack traces (`in /src/File.cs:line 42`), `path:line:col` and plain paths open at
  that line.

### Git

Through the `git` CLI, so your config, credentials and hooks apply.

- a Source Control view (`Ctrl+Shift+G`) with a commit box, staged and unstaged changes, stage/unstage/discard
  per file or all at once, and pull, push, fetch and sync
- an activity bar badge with the number of changes, plus the branch and ahead/behind in the status bar; click
  the branch to check out or create one
- change markers in the editor gutter (added, modified, deleted lines, compared with HEAD)
- colored names and status letters in the explorer, and a diff tab for any change ("Git: Open Changes")
- unmodified editors reload when a pull, checkout or discard changes their files on disk

### Docker

Docker support comes from the preinstalled **Docker** module (without it, ByteForge has no Docker support).

- Dockerfile highlighting and file icons
- a Docker view with the workspace's Dockerfiles and Compose files (build, compose up/down)
- containers grouped by Compose project (start, stop, pause, restart, logs, shell, inspect, remove; expand one
  to see its image, ports, volumes, networks and command)
- images (run, inspect, remove), volumes (which containers use them, inspect, remove, prune) and networks
- when Docker isn't usable, a SETUP section says why (engine stopped, not in the `docker` group, Compose or Buildx
  missing) with a button that fixes it in a terminal; after joining the group, the module runs docker through
  `sg docker` until you log in again
- "Docker: Add Docker Support to Project…" generates a multi-stage Dockerfile and `.dockerignore` for a .NET
  project, like Visual Studio; "Add Docker Compose File" a `compose.yaml` for them

Interactive commands run in the terminal. Set `"docker.composeCommand": "docker-compose"` for the standalone tool.

### Workbench and terminal

- a custom title bar with menus, a search box ("command center"), layout toggles and window controls
- an activity bar, a lazily expanded file explorer with create/rename/delete, and search in files. The file
  you're editing is highlighted with an accent bar, its folders are shown in bold, and the tree follows
  as you switch tabs.
- tabs, breadcrumbs, and Problems and Output panels
- **split editor:** Ctrl+\\ moves the current tab into a second editor group beside the first (and back);
  Ctrl+1 / Ctrl+2 focus the groups. Closing a group's last tab closes the group.
- a status bar, a command palette (`Ctrl+Shift+P`), go to file (`Ctrl+P`) and go to line (`Ctrl+G`)
- a theme picker with live preview and color swatches (`Ctrl+K`)
- **integrated terminal** (`` Ctrl+` ``): your shell (`$SHELL`, e.g. fish) on a real PTY, run by Alacritty's
  terminal engine and drawn in the panel. It supports 256 and true colors, scrollback, mouse selection,
  Ctrl+Shift+C/V copy and paste (middle-click pastes), and several terminals at once. Shell keys like Ctrl+R,
  Ctrl+W and Ctrl+K go to the shell while the terminal is focused.

---

## Getting started

### Requirements

- Rust (stable, 1.85+)
- the .NET SDK
- the C# language server: `dotnet tool install -g csharp-ls`
- for debugging, [netcoredbg](https://github.com/Samsung/netcoredbg): on Arch, the AUR package `netcoredbg`;
  elsewhere, unpack a release to `~/.local/share/netcoredbg/` or set `"debug.netcoredbgPath"`
- for ASP.NET Core, Razor and Blazor projects, the ASP.NET Core runtime and targeting pack (Arch: `aspnet-runtime`
  and `aspnet-targeting-pack`)

### Run from source

```sh
cargo run --release -p byteforge -- ~/code/my-project
# a folder plus files to open in tabs
cargo run --release -p byteforge -- ~/code/my-project ~/code/my-project/Program.cs
```

### Arch Linux

The easiest way is the ByteForge pacman repo (the `packages` branch). Add this to `/etc/pacman.conf`:

```ini
[byteforge]
SigLevel = Optional TrustAll
Server = https://raw.githubusercontent.com/DevStarByte/ByteForge/packages/$arch
```

then `sudo pacman -Sy byteforge` (updates come with `sudo pacman -Syu`). To build from source instead:

```sh
sudo pacman -S rust dotnet-sdk vulkan-icd-loader libxkbcommon libxkbcommon-x11 wayland fontconfig cmake clang
dotnet tool install -g csharp-ls
./packaging/linux/install.sh          # installs to ~/.local, adds `byteforge` and a desktop entry
# or build and install a pacman package (removable with `sudo pacman -R byteforge`):
cd packaging/arch && makepkg -si
```

After pulling changes, `makepkg -sfi` in `packaging/arch` rebuilds and reinstalls (incrementally; `makepkg -C`
starts from scratch). The package puts ByteForge in `/usr/lib/byteforge`, `byteforge` on PATH and a desktop entry.

> [!NOTE]
> GPUI renders through Vulkan on Linux, so you also need the Vulkan driver for your GPU (`vulkan-radeon`,
> `vulkan-intel` or `nvidia-utils`). On a tiling window manager, set `"titleBar.style": "native"` in your theme's
> `layout` section to keep server-side decorations (takes effect on the next start).

---

## Themes

Themes are JSON files (comments allowed) in two places:

- built-in: `<install dir>/themes/`
- user: `~/.config/byteforge/themes/` (Windows: `%APPDATA%\byteforge\themes\`)

Saving a theme file applies it immediately. **Preferences → Customize Current Theme** creates a user theme
that inherits the current one and opens it in the editor. Every theme inherits the default **Klarheit** layout
from the base theme.

| Theme | What it shows off |
|---|---|
| ByteForge Dark / Light | the reference themes; Dark documents every key |
| Neon Grid | synthwave gradients, neon glow shadows, rounded tabs, phase-blinking caret |
| Glass | translucent, blurred window background with pill tabs and activity items |
| Nord Frost | calm arctic palette, leading whitespace dots, ruler at column 100 |
| Phosphor | retro green CRT: monospace UI, block cursor, square corners, a custom logo icon |
| Rosé Dawn | warm pastel light theme, pill tabs, side bar on the right, underline cursor |
| Ember Compact | dense layout, fire gradients, no command center or breadcrumbs |
| Forge Midnight | a short partial theme: a few overrides plus a right-hand side bar |
| Neovim | Neovim's default colors and look: lualine-style status line with a NORMAL block, buffer tabs, no activity bar, block cursor, monospace UI |
| Flipside | layout showcase: activity bar inside the side bar, tabs at the bottom, panel as a right column, centered menus |

<details>
<summary><b>Theme sections reference</b></summary>

- **colors**: 127 keys covering every part of the workbench. Any color can be `#RGB`, `#RRGGBB`, `#RRGGBBAA`
  (CSS order), `transparent`, `linear-gradient(<angle>deg, c1, …, cN)`, or a reference like `@accent` or
  `@accent/12` (12 % opacity). GPUI gradients have two stops: the first and last colors are used.
- **tokenColors**: syntax colors by classification name (`class name`, `method name`, `keyword - control`,
  …), with optional bold, italic or underline. `a - b` falls back to `a`.
- **fonts**: families, sizes, weights and styles for the UI, menus, tabs, lists, status bar, welcome page and
  editor. `"default"` picks Inter, Segoe UI, Noto Sans and so on, whichever is installed.
- **metrics**: 99 keys for heights, widths, icon sizes, paddings, margins, spacings, border thicknesses,
  corner radii and opacities.
- **editor**: caret style, blink mode and speed, smooth-caret animation and speed, line highlight mode,
  selection radius, whitespace rendering, squiggle style, rainbow brackets, bracket matching, indent guides,
  rulers and paddings.
- **testing**: `testing.iconPassed`, `iconFailed`, `iconSkipped`, `iconQueued`, `iconUnset` (in `colors`).
- **minimap and groups**: `minimap.background`, `minimap.selectionHighlight`, `minimap.caretLine`,
  `minimapSlider.background` / `hoverBackground` and `editorGroup.border` (in `colors`), `minimap.width` (in
  `metrics`).
- **git colors** (in `colors`): `gitDecoration.*ResourceForeground`, `editorGutter.added/modified/deletedBackground`,
  `diffEditor.inserted/removedLineBackground`, `activityBarBadge.*`, `badge.*`; sizes in `metrics`:
  `editorGutter.changeWidth/changeOffset/changeRadius`, `scm.iconSize`, `activityBarBadge.size`.
- **effects**: window backdrop (`blur`, `transparent` or `none`) and shadows for the palette and other widgets.
- **layout**: where every part of the window goes. Each container is an ordered list of parts: reorder
  parts to move them, leave them out to hide them (see [Layout](#layout)).
- **icons**: replace any built-in icon (logo, files, search, window buttons, …) with SVG path data. Every file
  type has its own icon (a page with the type's symbol: `#` for C#, `{ }` for JSON, `< >` for markup, …), colored
  by `icon.csharp`, `icon.json`, `icon.razor`, `icon.yaml`, `icon.script` and the other `icon.*` keys. Modules can
  add icons for more file types (the preinstalled Docker module does this for Docker files).

</details>

---

## Layout

The `layout` section of a theme describes the whole window as ordered lists. Lists can be JSON arrays or
comma-separated strings:

```jsonc
"layout": {
  "workbench.order": ["titleBar", "main", "statusBar"],          // rows; the title bar can also go to the bottom
  "main.order": ["activityBar", "sideBar", "editor"],              // columns; add "panel" for a side panel
  "editor.order": ["tabs", "breadcrumbs", "content", "panel"],     // e.g. ["content", "tabs"] puts tabs at the bottom
  "sideBar.order": ["header", "content"],                          // add "activityBar" for a horizontal one on top
  "titleBar.left": ["logo", "menu"], "titleBar.center": ["commandCenter"],
  "titleBar.right": ["layoutControls", "windowControls"],
  "activityBar.items": ["explorer", "search", "scm", "modules", "spacer", "settings"],
  "statusBar.left": ["branch", "sync", "problems", "run", "modules"], "statusBar.center": [],
  "statusBar.right": ["modules", "position", "indent", "encoding", "eol", "language", "lsp"],
  "panel.tabs": ["problems", "output"], "menuBar.menus": ["File", "Edit", "View", "Go", "Git", "Run", "Preferences", "Help"],
  "welcome.order": ["logo", "title", "shortcuts"], "welcome.title": "ByteForge",
  "welcome.shortcuts": ["file.openFolder", "view.quickOpen", "view.commandPalette", "theme.select", "dotnet.build"],
  "sideBar.visible": true, "panel.visible": false, "sideBar.defaultView": "explorer", "tab.indicatorPosition": "top"
}
```

- **Title bar parts:** logo, menu, title, commandCenter, branch, layoutControls (toggleSideBar, togglePanel,
  themePicker), windowControls, spacer.
- **Status bar parts:** branch, sync, file, problems, errors, warnings, run, modules, position, indent, encoding,
  eol, language, lsp, spacer, and `text:<anything>` for fixed text. Each part can have its own colors and weight:
  `statusBarItem.<part>.background/foreground` (colors) and `statusBarItem.<part>.weight` (fonts).
  `statusBar.positionFormat` sets the cursor position text (`{line}`, `{col}`, `{lines}`).
- **Any command anywhere:** the title bar, activity bar and status bar all accept any command:
  `"command:dotnet.build:build"` (command id plus an icon name), `"command:git.pull"` or just `"git.pull"`.
- **Safety rules:** the editor and the side bar's content can be moved but not removed. A custom title bar always
  keeps its window buttons.
- **Older keys:** `sideBar.position`, `activityBar.position`, `tabs.visible`, `statusBar.visible` and the other
  older keys still work as defaults when the lists aren't set.

`themes/byteforge-dark.json` documents every key, and `themes/flipside.json` shows a rearranged window.

---

## Settings and keybindings

`~/.config/byteforge/settings.json` (**Preferences → Open Settings**) reloads live:

```jsonc
{
  "workbench.theme": "ByteForge Dark",
  "editor.tabSize": 4, "editor.insertSpaces": true, "editor.smoothScrolling": true, "editor.minimap": true,
  "files.exclude": ["bin", "obj", ".git", "node_modules", "target"],
  "csharp.languageServer": "",                         // custom path to csharp-ls (optional)
  "dotnet.project": "src/MyApp/MyApp.csproj",          // build/run/debug target (optional)
  "dotnet.launchProfile": "https",                     // launchSettings.json profile (default: first "Project" one)
  "debug.netcoredbgPath": "",                          // custom path to netcoredbg (optional)
  "debug.justMyCode": true, "debug.stopAtEntry": false, "debug.openBrowser": true,
  "debug.breakOnExceptions": ["user-unhandled"],       // or ["all"], or []
  "terminal.shell": "/usr/bin/fish", "terminal.args": [],  // default: your login shell ($SHELL); also set by
                                                     // "Terminal: Select Default Shell…" (palette or the panel button)
  "editor.formatOnSave": false, "editor.defaultFormatter": "csharpier",   // or "csharp-ls"
  "explorer.autoReveal": true,                         // expand and scroll the tree to the file you switch to
  "keybindings": { "view.quickOpen": "ctrl-e", "theme.select": "" },   // "" unbinds
  "modules.disabled": ["me.hello"]
}
```

<details open>
<summary><b>Default shortcuts</b></summary>

| Shortcut | Action | Shortcut | Action |
|---|---|---|---|
| Ctrl+Shift+P / F1 | Command palette | Ctrl+P | Go to file |
| Ctrl+Shift+O | Open folder | Ctrl+G | Go to line |
| Ctrl+S / Ctrl+Shift+S | Save / Save as | F12, Ctrl+Click | Go to definition |
| Ctrl+F / Ctrl+H | Find / Replace | Ctrl+Space | Completion |
| Ctrl+/ (Ctrl+# on DE) | Toggle comment | Alt+↑/↓ | Move lines |
| Ctrl+D | Add next occurrence (multi-cursor) | Ctrl+Shift+K | Delete lines |
| Alt+Click, Ctrl+Alt+↑/↓ | Add cursor | Shift+Alt+↓ | Duplicate lines |
| Ctrl+\\ | Split editor | Ctrl+1 / Ctrl+2 | Focus editor group |
| Ctrl+B / Ctrl+J | Toggle side bar / panel | Ctrl+Shift+E / F | Explorer / Search |
| Ctrl+Shift+B | Build | F5 | Start debugging / Continue |
| Ctrl+F5 | Run without debugging | Shift+F5 | Stop |
| F9 | Toggle breakpoint | F10 / F11 / Shift+F11 | Step over / into / out |
| F6 | Pause | Ctrl+Shift+F5 | Restart debugging |
| Ctrl+Shift+D | Run and Debug | Ctrl+Shift+Y | Debug Console |
| Ctrl+\` | Toggle terminal | Ctrl+Shift+\` | New terminal |
| Ctrl+. | Quick fix / refactor at the cursor | Shift+Alt+F | Format document |
| Ctrl+K | Color theme | Ctrl+Tab | Next editor |
| Ctrl+Shift+G | Source Control | | |

</details>

---

## Modules

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

### Writing a module

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

<details>
<summary><b>Module protocol</b> (one JSON object per line over stdio)</summary>

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

</details>

---

## Architecture

```
crates/
  bf-text     rope buffer, undo, and the editing model (caret, selection, every editing command); UI-free and tested
  bf-syntax   tree-sitter highlighting (C#, JSON, Razor, XML, YAML) and modules' regex languages, emitting theme token names; incremental and viewport-ranged
  bf-lsp      LSP client (std threads + flume channels, no async runtime); tested live against csharp-ls
  bf-dap      Debug Adapter Protocol client in the same style; tested live against netcoredbg
  bf-core     paths, settings, themes (inheritance, hot reload), workspace index/watcher, search, dotnet runner,
              debug helpers (launch profiles, target assembly, build diagnostics), module host
  byteforge   the GPUI app: workbench, editor element, palette, explorer, search, theme → GPUI mapping, icons
modules/      preinstalled modules (CSharpier, Docker)
themes/       built-in themes (byteforge-dark.json documents every key)
packaging/    Linux install script, desktop entry, Arch PKGBUILD for local builds
              (the repo's PKGBUILD and build workflow live on the `packages` branch)
```

### Performance

- Edits are O(log n) in the rope.
- Highlighting runs on background threads over a window around the viewport (about 3 ms for a 32 KB window of
  a 60k-line file) and is applied only if the document hasn't changed in the meantime. Existing colors are
  shifted while you type, so nothing flickers.
- Only visible lines are shaped, and shaped lines are cached by content and colors.
- csharp-ls runs out of process, so a slow project load never blocks the UI.

---

## Limitations and next steps

- **Build:** the Linux build compiles on Arch. `libc` is pinned to 0.2.189 in `Cargo.lock` because newer versions
  removed `ENOATTR`, which `xattr` 0.2 (pulled in by GPUI) still needs.
- **Large solutions:** csharp-ls loads the project from the opened folder. Very large solutions take a while
  before diagnostics appear (the status bar shows progress).
- **IME:** composition is basic: committed text is inserted, but there's no inline pre-edit display.
- **Git:** pull is fast-forward only, there is no merge-conflict editor, no side-by-side diff and no blame yet.
- **Razor:** no language server yet, so there is no completion or hover in `.razor`/`.cshtml` files, and errors
  appear after a build. A `<` inside an inline `<script>` confuses the Razor grammar, so colors after it can be wrong.
- **Debugging:** no conditional breakpoints, watch expressions or data tips yet. Blazor WebAssembly code runs
  in the browser and can't be debugged; Blazor Server and the server side of hosted apps can.
- **Terminal:** mouse reporting for full-screen programs (clicking inside vim or htop) and IME pre-edit aren't
  supported yet.

See [ROADMAP.md](ROADMAP.md) for what's planned.

---

<div align="center">

Built with Rust and GPUI · MIT licensed

</div>

<!-- Rust badge with Ferris the crab (public domain, rustacean.net) as its logo -->
[rust-badge]: https://img.shields.io/badge/Rust-1.85%2B-orange?logo=data%3Aimage%2Fsvg%2Bxml%3Bbase64%2CPHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAxMjAwIDgwMCI%2BPGc%2BPGcgdHJhbnNmb3JtPSJ0cmFuc2xhdGUoNTk3IDYzNykiPjxwYXRoIGQ9Ik0wLC0yODBDLTEyMSwtMjgwLTIzMSwtMjY1LTMxMywtMjQxTC0zMTMsLTM4Qy0yMzEsLTE1LTEyMSwwIDAsMEMxMzksMCAyNjMsLTE5IDM0NiwtNDlMMzQ2LC0yMzBDMjYzLC0yNjAgMTM5LC0yODAgMCwtMjgwIiBmaWxsPSIjYTUyYjAwIi8%2BPC9nPjxnIHRyYW5zZm9ybT0idHJhbnNsYXRlKDEwNjkgNTc2KSI%2BPHBhdGggZD0iTTAsLTUzTC0xNCwtODNDLTE0LC04NC0xNCwtODUtMTQsLTg2Qy0xNCwtMTE5LTQ5LC0xNTAtMTA3LC0xNzVMLTEwNywzQy04MCwtOS01OCwtMjItNDIsLTM1Qy00NywtMTctNjMsMjEtNzUsNDhDLTk2LDg2LTEwNCwxMTktMTAzLDEyMUMtMTAyLDEyMi05NSwxMTEtODQsOTNDLTYwLDU4LTE0LC04LTUsLTI1QzYsLTQ0IDAsLTUzIDAsLTUzIiBmaWxsPSIjYTUyYjAwIi8%2BPC9nPjxnIHRyYW5zZm9ybT0idHJhbnNsYXRlKDE0OSA1OTEpIj48cGF0aCBkPSJNMCwtMTAwQzAsLTk0IDEsLTg3IDQsLTgxTC01LC02NkMtNSwtNjYtMTIsLTU3IDAsLTM4QzExLC0yMiA2NCw0MyA5Miw3NkMxMDQsOTQgMTEyLDEwNCAxMTMsMTAzQzExNCwxMDIgMTA2LDY5IDgyLDMyQzcwLDEyIDU3LC0xNCA0OSwtMzRDNzEsLTIwIDEwMSwtNyAxMzUsNEwxMzUsLTIwNEM1MiwtMTc4IDAsLTE0MSAwLC0xMDAiIGZpbGw9IiNhNTJiMDAiLz48L2c%2BPGcgdHJhbnNmb3JtPSJ0cmFuc2xhdGUoMTE1MSAyODIpIj48cGF0aCBkPSJNMCwyNDBMLTkzLDE3MkMtOTQsMTY4LTk1LDE2NS05NiwxNjJMLTY1LDExOEMtNjIsMTEzLTYyLDEwOC02NCwxMDJDLTY2LDk3LTcwLDk0LTc1LDkzTC0xMjcsODRDLTEyOSw4MC0xMzEsNzYtMTM0LDcyTC0xMTIsMjJDLTExMCwxNy0xMTAsMTEtMTEzLDZDLTExNiwyLTEyMSwtMS0xMjYsLTFMLTE3OSwxQy0xODIsLTMtMTg0LC02LTE4NywtMTBMLTE3NSwtNjNDLTE3NCwtNjgtMTc1LC03NC0xNzksLTc4Qy0xODMsLTgyLTE4OSwtODQtMTk0LC04MkwtMjQ1LC03MEMtMjQ4LC03My0yNTIsLTc2LTI1NSwtNzhMLTI1MywtMTMzQy0yNTMsLTEzOS0yNTYsLTE0NC0yNjAsLTE0N0MtMjY1LC0xNTAtMjcwLC0xNTEtMjc1LC0xNDlMLTMyMywtMTI2Qy0zMjcsLTEyOC0zMzEsLTEzMC0zMzUsLTEzMkwtMzQzLC0xODZDLTM0NCwtMTkyLTM0NywtMTk3LTM1MiwtMTk5Qy0zNTcsLTIwMS0zNjMsLTIwMC0zNjcsLTE5N0wtNDEwLC0xNjVDLTQxNCwtMTY2LTQxOCwtMTY4LTQyMywtMTY5TC00NDEsLTIyMEMtNDQzLC0yMjUtNDQ3LC0yMjktNDUyLC0yMzBDLTQ1OCwtMjMxLTQ2MywtMjMwLTQ2NywtMjI2TC01MDMsLTE4NUMtNTA3LC0xODYtNTExLC0xODYtNTE2LC0xODdMLTU0MywtMjMzQy01NDYsLTIzOC01NTEsLTI0MS01NTYsLTI0MUMtNTYyLC0yNDEtNTY3LC0yMzgtNTcwLC0yMzNMLTU5NywtMTg3Qy02MDIsLTE4Ni02MDYsLTE4Ni02MTAsLTE4NUwtNjQ2LC0yMjZDLTY1MCwtMjMwLTY1NSwtMjMxLTY2MSwtMjMwQy02NjYsLTIyOS02NzAsLTIyNS02NzIsLTIyMEwtNjkwLC0xNjlDLTY5NSwtMTY4LTY5OSwtMTY2LTcwMywtMTY1TC03NDYsLTE5N0MtNzUwLC0yMDAtNzU2LC0yMDEtNzYxLC0xOTlDLTc2NiwtMTk3LTc2OSwtMTkyLTc3MCwtMTg2TC03NzgsLTEzMkMtNzgyLC0xMzAtNzg2LC0xMjgtNzkwLC0xMjZMLTgzOCwtMTQ5Qy04NDMsLTE1MS04NDgsLTE1MC04NTMsLTE0N0MtODU3LC0xNDQtODYwLC0xMzktODYwLC0xMzNMLTg1OCwtNzhDLTg2MSwtNzYtODY1LC03My04NjgsLTcwTC05MTksLTgyQy05MjQsLTg0LTkzMCwtODItOTM0LC03OEMtOTM3LC03NC05MzksLTY4LTkzOCwtNjNMLTkyNiwtMTBDLTkyOCwtNi05MzEsLTMtOTM0LDFMLTk4NywtMUMtOTkyLC0xLTk5NywyLTEwMDAsNkMtMTAwMywxMS0xMDAzLDE3LTEwMDEsMjJMLTk3OSw3MkMtOTgyLDc2LTk4NCw4MC05ODYsODRMLTEwMzgsOTNDLTEwNDMsOTQtMTA0Nyw5Ny0xMDQ5LDEwMkMtMTA1MSwxMDgtMTA1MSwxMTMtMTA0OCwxMThMLTEwMTcsMTYyQy0xMDE3LDE2My0xMDE3LDE2NC0xMDE4LDE2NUwtMTEwNCwyNTdDLTExMDQsMjU3LTExMTgsMjY3LTEwOTgsMjkyQy0xMDgxLDMxMy05OTQsMzk5LTk0Nyw0NDRDLTkyOCw0NjctOTE0LDQ4MS05MTMsNDgwQy05MTEsNDc4LTkyMiw0MzYtOTYxLDM4OEMtOTkwLDM0NC0xMDI5LDI3Ny0xMDIwLDI2OUMtMTAyMCwyNjktMTAxMCwyNTYtOTg5LDI0N0MtOTg4LDI0Ny05OTAsMjQ2LTk4OSwyNDdDLTk4OSwyNDctNTU0LDQ0Ny0xNTAsMjUwQy0xMDQsMjQyLTc2LDI2Ny03NiwyNjdDLTY3LDI3Mi05MiwzNDEtMTEyLDM4N0MtMTQwLDQzOS0xNDQsNDc5LTE0Miw0ODFDLTE0MCw0ODEtMTMwLDQ2Ni0xMTYsNDQxQy04MCwzOTItMTIsMjk3IDAsMjc0QzE0LDI0OCAwLDI0MCAwLDI0MCIgZmlsbD0iI2Y3NGMwMCIvPjwvZz48ZyB0cmFuc2Zvcm09InRyYW5zbGF0ZSg0NTAgNDg0KSI%2BPHBhdGggZD0iTTAsMTY3Qy0yLDE2Ni0zLDE2NS0zLDE2NUwxNDAsMTUzQzI0LDAtNjksMTIyLTc5LDEzNkwtNzksMTY3TDAsMTY3WiIgc3R5bGU9ImZpbGwtcnVsZTpub256ZXJvOyIvPjwvZz48ZyB0cmFuc2Zvcm09InRyYW5zbGF0ZSg3NDcgNDc3KSI%2BPHBhdGggZD0iTTAsMTcyQzIsMTcxIDMsMTcwIDMsMTcwTC0xMzQsMTYwQy0xOCwwIDY5LDEyNyA3OSwxNDBMNzksMTcyTDAsMTcyWiIgc3R5bGU9ImZpbGwtcnVsZTpub256ZXJvOyIvPjwvZz48ZyB0cmFuc2Zvcm09InRyYW5zbGF0ZSg5NjYgODExKSI%2BPHBhdGggZD0iTTAsLTMxNEMwLC0zMTQtMTYsLTI1Mi0xMTIsLTE4N0wtMTQwLC0xODBDLTE0MCwtMTgwLTIyOCwtMzQxLTM1MiwtMTYwQy0zNTIsLTE2MC0zMTMsLTE4My0yMDksLTE1NUMtMjA5LC0xNTUtMjU3LC04Mi0zNTMsLTg0Qy0zNTMsLTg0LTI2MSwyNy0xMjAsLTEzM0MtMTIwLC0xMzMgMjgsLTE5MSA0MCwtMzE0TDAsLTMxNFoiIGZpbGw9IiNmNzRjMDAiLz48L2c%2BPGcgdHJhbnNmb3JtPSJ0cmFuc2xhdGUoNjc3IDUxMCkiPjxwYXRoIGQ9Ik0wLC05MkMwLC05MiA0MywtMTQwIDg3LC05MkM4NywtOTIgMTIxLC0yOSA4NywzQzg3LDMgMzEsNDggMCwzQzAsMy0zNywtMzIgMCwtOTIiIHN0eWxlPSJmaWxsLXJ1bGU6bm9uemVybzsiLz48L2c%2BPGcgdHJhbnNmb3JtPSJ0cmFuc2xhdGUoNzI4IDQzNSkiPjxwYXRoIGQ9Ik0wLDBDMCwxOS0xMSwzNC0yNCwzNEMtMzgsMzQtNDksMTktNDksMEMtNDksLTE5LTM4LC0zNC0yNCwtMzRDLTExLC0zNCAwLC0xOSAwLDAiIGZpbGw9IiNmZmYiLz48L2c%2BPGcgdHJhbnNmb3JtPSJ0cmFuc2xhdGUoNDgzIDUwMykiPjxwYXRoIGQ9Ik0wLC05OEMwLC05OCA3NSwtMTMxIDk1LC01OEM5NSwtNTggMTE2LDI4IDM0LDMzQzM0LDMzLTcyLDEzIDAsLTk4IiBzdHlsZT0iZmlsbC1ydWxlOm5vbnplcm87Ii8%2BPC9nPjxnIHRyYW5zZm9ybT0idHJhbnNsYXRlKDUyMSA0MzYpIj48cGF0aCBkPSJNMCwwQzAsMTktMTEsMzUtMjUsMzVDLTM5LDM1LTUwLDE5LTUwLDBDLTUwLC0xOS0zOSwtMzUtMjUsLTM1Qy0xMSwtMzUgMCwtMTkgMCwwIiBmaWxsPSIjZmZmIi8%2BPC9nPjxnIHRyYW5zZm9ybT0idHJhbnNsYXRlKDQ0MSA2ODgpIj48cGF0aCBkPSJNMCwtMjVDOTIsLTM3IDE0NSwtMzggMTQ1LC0zOEMyMiwtMjAwLTc4LC01My03OCwtNTNDLTEwMywtNjItMTI4LC05Ny0xNDcsLTEyOUwtMjIzLC0xNTFDLTEzNiwtMi03MCwwLTcwLDBDNjYsMTc1IDEzMSwzNCAxMzEsMzRDNTQsMjYgMCwtMjUgMCwtMjUiIGZpbGw9IiNmNzRjMDAiLz48L2c%2BPC9nPjwvc3ZnPg%3D%3D
