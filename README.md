# ByteForge

A modular, from-scratch, VS Code–style IDE for **C#**, written in **Rust** with **GPUI** (the GPU-accelerated
UI framework behind Zed). It runs on Linux (including Arch, on X11 and Wayland) and Windows. Every part of the
UI is styled from theme files that reload live.

## Features

- **Custom editor:** a rope-backed buffer (`ropey`) that renders only the visible lines and caches the
  shaped text per line.
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
- **C# intelligence** via the **csharp-ls** language server (Roslyn-based, LSP): semantic highlighting,
  diagnostics, completion, Markdown hover, go-to-definition (F12 / Ctrl+Click), **Find All References** (Shift+F12, in
  a References panel grouped by file), **Go to Implementations** (Ctrl+F12) and **Type Definition**, **Go to Symbol**
  in the file ("@" in Go to File) or the whole workspace (Ctrl+T, "#"), **Rename Symbol** (F2, across files), **parameter hints** while typing a call
  (Ctrl+Shift+Space; the arrows switch overloads) and **quick fixes**: Ctrl+. on an
  error or warning lists the fixes (add a `using`, generate a type, …) and refactorings; picking one applies it,
  also across files.
- **Razor** (`.razor` components, `.cshtml` pages and views): tree-sitter highlighting of directives, control
  flow, embedded C# and HTML markup, `@* *@` comment toggling, and Razor compile errors from `dotnet build` as
  squiggles and in Problems.
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
- **Formatting**: Format Document (Shift+Alt+F), optional format on save. **CSharpier** is the default
  formatter for C# and MSBuild/XML files. It's a built-in module you can remove (and restore) in the Modules view; without it, or with
  `"editor.defaultFormatter": "csharp-ls"`, csharp-ls (Roslyn) formats C#. "Format Document With…" switches.
- **Integrated terminal** (`` Ctrl+` ``): your shell (`$SHELL`, e.g. fish) on a real PTY, run by Alacritty's
  terminal engine and drawn in the panel. It supports 256 and true colors, scrollback, mouse selection,
  Ctrl+Shift+C/V copy and paste (middle-click pastes), and several terminals at once. Shell keys like Ctrl+R,
  Ctrl+W and Ctrl+K go to the shell while the terminal is focused.
- **Docker**: Dockerfile and Compose (YAML) highlighting, and a Docker view with the workspace's Dockerfiles and
  Compose files (build, compose up/down), containers grouped by Compose project (start, stop, restart, logs, shell,
  remove) and images (run, remove). "Docker: Add Docker Support to Project…" generates a multi-stage Dockerfile and
  `.dockerignore` for a .NET project, like Visual Studio; "Add Docker Compose File" a `compose.yaml` for them.
  Interactive commands run in the terminal. Set `"docker.composeCommand": "docker-compose"` for the standalone tool.
- **XML** highlighting for MSBuild and .NET files (`.csproj`, `.props`, `.targets`, `.slnx`, `.config`, `.resx`,
  `.xaml`, `.nuspec`, …), with MSBuild references (`$(Property)`, `@(Item)`, `%(Metadata)`) in their own color and
  `<!-- -->` comment toggling.
- **Instant syntax colors** from **tree-sitter**, computed around the viewport on background threads.
  Semantic colors are layered on top as they arrive.
- **Workbench:**
  - a custom title bar with menus, a search box ("command center"), layout toggles and window controls
  - an activity bar, a lazily expanded file explorer with create/rename/delete, and search in files. The file
    you're editing is highlighted with an accent bar, its folders are shown in bold, and the tree follows
    as you switch tabs.
  - tabs, breadcrumbs, and Problems and Output panels
  - **split editor:** Ctrl+\\ moves the current tab into a second editor group beside the first (and back);
    Ctrl+1 / Ctrl+2 focus the groups. Closing a group's last tab closes the group.
  - a status bar, a command palette (`Ctrl+Shift+P`), go to file (`Ctrl+P`) and go to line (`Ctrl+G`)
  - a theme picker with live preview and color swatches (`Ctrl+K`).
- **Git** (through the `git` CLI, so your config, credentials and hooks apply):
  - a Source Control view (`Ctrl+Shift+G`) with a commit box, staged and unstaged changes, stage/unstage/discard
    per file or all at once, and pull, push, fetch and sync
  - an activity bar badge with the number of changes, plus the branch and ahead/behind in the status bar; click
    the branch to check out or create one
  - change markers in the editor gutter (added, modified, deleted lines, compared with HEAD)
  - colored names and status letters in the explorer, and a diff tab for any change ("Git: Open Changes")
  - unmodified editors reload when a pull, checkout or discard changes their files on disk
- **.NET CLI**: build, run, test, clean and restore, with output streamed into the Output panel and compiler
  errors collected in Problems.
- **Clickable file references** in Output, the Debug Console (click) and the terminal (Ctrl+Click): MSBuild
  errors (`File.cs(12,5)`), .NET stack traces (`in /src/File.cs:line 42`), `path:line:col` and plain paths open at
  that line.
- **Modules**: out-of-process plugins in any language that speak JSON over stdio. They add commands, key
  bindings and status bar items, and they receive workspace events. A broken module can't crash the IDE.

## Running

You need Rust (stable, 1.85+), the .NET SDK, and the C# language server:

```sh
dotnet tool install -g csharp-ls
cargo run --release -p byteforge -- ~/code/my-project
# a folder plus files to open in tabs
cargo run --release -p byteforge -- ~/code/my-project ~/code/my-project/Program.cs
```

For debugging you also need [netcoredbg](https://github.com/Samsung/netcoredbg): on Arch, use the AUR package
`netcoredbg`. Elsewhere, unpack a release to `~/.local/share/netcoredbg/`, or set `"debug.netcoredbgPath"`. ASP.NET
Core, Razor and Blazor projects also need the ASP.NET Core runtime and targeting pack (Arch: `aspnet-runtime`
and `aspnet-targeting-pack`).

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

GPUI renders through Vulkan on Linux, so you also need the Vulkan driver for your GPU (`vulkan-radeon`,
`vulkan-intel` or `nvidia-utils`). If you use a tiling window manager, set `"titleBar.style": "native"` in
your theme's `layout` section to keep server-side decorations. That setting takes effect on the next start.

## Architecture

```
crates/
  bf-text     rope buffer, undo, and the editing model (caret, selection, every editing command); UI-free and tested
  bf-syntax   tree-sitter highlighting (C#, JSON, Razor, XML) emitting theme token names; incremental and viewport-ranged
  bf-lsp      LSP client (std threads + flume channels, no async runtime); tested live against csharp-ls
  bf-dap      Debug Adapter Protocol client in the same style; tested live against netcoredbg
  bf-core     paths, settings, themes (inheritance, hot reload), workspace index/watcher, search, dotnet runner,
              debug helpers (launch profiles, target assembly, build diagnostics), module host
  byteforge   the GPUI app: workbench, editor element, palette, explorer, search, theme → GPUI mapping, icons
themes/       built-in themes (byteforge-dark.json documents every key)
samples/      hello-module (Python) — an example out-of-process module
packaging/    Linux install script, desktop entry, Arch PKGBUILD for local builds
              (the repo's PKGBUILD and build workflow live on the `packages` branch)
```

Performance notes:

- Edits are O(log n) in the rope.
- Highlighting runs on background threads over a window around the viewport (about 3 ms for a 32 KB window of
  a 60k-line file) and is applied only if the document hasn't changed in the meantime. Existing colors are
  shifted while you type, so nothing flickers.
- Only visible lines are shaped, and shaped lines are cached by content and colors.
- csharp-ls runs out of process, so a slow project load never blocks the UI.

## Themes: customize every part of the UI

Themes are JSON files (comments allowed) in two places:

- built-in: `<install dir>/themes/`
- user: `~/.config/byteforge/themes/` (Windows: `%APPDATA%\byteforge\themes\`)

Saving a theme file applies it immediately. **Preferences → Customize Current Theme** creates a user theme
that inherits the current one and opens it in the editor.

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

The theme sections:

- **colors**: 127 keys covering every part of the workbench. Any color can be `#RGB`, `#RRGGBB`, `#RRGGBBAA`
  (CSS order), `transparent`, or `linear-gradient(<angle>deg, c1, …, cN)`. GPUI gradients have two stops:
  the first and last colors are used.
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
  parts to move them, leave them out to hide them (see the next section).
- **icons**: replace any built-in icon (logo, files, search, window buttons, …) with SVG path data.

## Layout: arrange every part of the window

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
  "modules.disabled": ["sample.hello"]
}
```

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
| Ctrl+` | Toggle terminal | Ctrl+Shift+` | New terminal |
| Ctrl+. | Quick fix / refactor at the cursor | Shift+Alt+F | Format document |
| Ctrl+K | Color theme | Ctrl+Tab | Next editor |
| Ctrl+Shift+G | Source Control | | |

## Writing a module

A module is a folder in `~/.config/byteforge/modules/` (or `<install dir>/modules/`) with a `module.json`:

```json
{ "id": "me.hello", "name": "Hello", "command": "python", "args": ["hello.py"] }
```

ByteForge starts the command in that folder and exchanges one JSON object per line:

- **Module → IDE:**
  - `{"type":"registerCommand","id","title","category","keybinding"}`
  - `{"type":"registerFormatter","id","name","extensions":["cs","csproj"]}`: become a formatter for these files
  - `{"type":"formatResult","requestId","text"}` or `{…,"error"}`: the answer to a `format` request
  - `{"type":"status","id","text","tooltip","command","alignment":"left|right"}`
  - `{"type":"log","text"}`
  - `{"type":"openFile","path","line","column"}`
  - `{"type":"showView","id"}`
- **IDE → module:**
  - `{"type":"activate","workspace"}`
  - `{"type":"executeCommand","id"}`
  - `{"type":"format","requestId","path","text"}` (formatter modules only)
  - `{"type":"event","name":"folderOpened|fileSaved|themeChanged","path"}`

`samples/hello-module` is a complete Python example: a command with a key binding plus a status bar clock.
`modules/csharpier` is the built-in formatter module. Built-in modules live in `<install dir>/modules/`; a user module with
the same id replaces one. The Modules view removes modules (built-in ones are only turned off and can be restored) and
uninstalls user modules.

The CSharpier module uses a version pinned in the project's dotnet tool manifest (`dotnet tool install csharpier`
in the repository), else a global one. Without either, the status bar offers to run `dotnet tool install -g csharpier`.
It needs `python3`.

## Limitations and next steps

- The Linux build compiles on Arch. `libc` is pinned to 0.2.189 in `Cargo.lock` because newer versions removed
  `ENOATTR`, which `xattr` 0.2 (pulled in by GPUI) still needs.
- csharp-ls loads the project from the opened folder. Very large solutions take a while before diagnostics
  appear (the status bar shows progress).
- IME composition is basic: committed text is inserted, but there's no inline pre-edit display.
- Git: pull is fast-forward only, there is no merge-conflict editor, no side-by-side diff and no blame yet.
- Razor has no language server yet: there is no completion or hover in `.razor`/`.cshtml` files, and errors appear
  after a build. A `<` inside an inline `<script>` confuses the Razor grammar, so colors after it can be wrong.
- Debugging: there are no conditional breakpoints, watch expressions or data tips yet. Blazor WebAssembly code runs
  in the browser and can't be debugged; Blazor Server and the server side of hosted apps can.
- Terminal: mouse reporting for full-screen programs (clicking inside vim or htop) and IME pre-edit aren't
  supported yet.
- Not implemented yet: folding, minimap, multi-cursor and
  split editors. Modules can't add custom UI views yet, only commands, status items and logs.
