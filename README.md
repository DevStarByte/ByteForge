# ByteForge

A modular, from-scratch, VS Code–style IDE for **C#**, written in **Rust** with **GPUI** (the GPU-accelerated
UI framework behind Zed). It runs on Linux (including Arch, on X11 and Wayland) and Windows. Every part of the
UI is styled from theme files that reload live.

## Features

- **Custom editor:** a rope-backed buffer (`ropey`) that renders only the visible lines and caches the
  shaped text per line.
  - **Editing:** undo/redo that groups typing, auto-indent, bracket/quote auto-close, comment toggling,
    move/duplicate/delete lines, find & replace, and word/line selection by double/triple click.
  - **Look:** smooth scrolling, an animated gliding caret (line, block, outline or underline style; blink,
    smooth, phase or solid), rainbow brackets, matching-bracket highlight, indent guides, visible whitespace,
    rulers and diagnostic squiggles.
- **C# intelligence** via the **csharp-ls** language server (Roslyn-based, LSP): semantic highlighting,
  diagnostics, completion, hover and go-to-definition (F12 / Ctrl+Click).
- **Instant syntax colors** from **tree-sitter**, computed around the viewport on background threads.
  Semantic colors are layered on top as they arrive.
- **Workbench:**
  - a custom title bar with menus, a search box ("command center"), layout toggles and window controls
  - an activity bar, a lazily expanded file explorer with create/rename/delete, and search in files
  - tabs, breadcrumbs, and Problems and Output panels
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
- **.NET CLI**: build, run, test, clean and restore, with output streamed into the Output panel.
- **Modules**: out-of-process plugins in any language that speak JSON over stdio. They add commands, key
  bindings and status bar items, and they receive workspace events. A broken module can't crash the IDE.

## Running

You need Rust (stable, 1.85+), the .NET SDK, and the C# language server:

```sh
dotnet tool install -g csharp-ls
cargo run --release -p byteforge -- ~/code/my-project
```

### Arch Linux

```sh
sudo pacman -S rust dotnet-sdk vulkan-icd-loader libxkbcommon libxkbcommon-x11 wayland fontconfig cmake clang
dotnet tool install -g csharp-ls
./packaging/linux/install.sh          # installs to ~/.local, adds `byteforge` and a desktop entry
# or build a package:  cd packaging/arch && makepkg -si
```

GPUI renders through Vulkan on Linux, so you also need the Vulkan driver for your GPU (`vulkan-radeon`,
`vulkan-intel` or `nvidia-utils`). If you use a tiling window manager, set `"titleBar.style": "native"` in
your theme's `layout` section to keep server-side decorations. That setting takes effect on the next start.

## Architecture

```
crates/
  bf-text     rope buffer, undo, and the editing model (caret, selection, every editing command); UI-free and tested
  bf-syntax   tree-sitter highlighting (C#, JSON) emitting theme token names; incremental and viewport-ranged
  bf-lsp      LSP client (std threads + flume channels, no async runtime); tested live against csharp-ls
  bf-core     paths, settings, themes (inheritance, hot reload), workspace index/watcher, search, dotnet runner, module host
  byteforge   the GPUI app: workbench, editor element, palette, explorer, search, theme → GPUI mapping, icons
themes/       built-in themes (byteforge-dark.json documents every key)
samples/      hello-module (Python) — an example out-of-process module
packaging/    Linux install script, desktop entry, Arch PKGBUILD
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
  "editor.tabSize": 4, "editor.insertSpaces": true, "editor.smoothScrolling": true,
  "files.exclude": ["bin", "obj", ".git", "node_modules", "target"],
  "csharp.languageServer": "",                         // custom path to csharp-ls (optional)
  "dotnet.project": "src/MyApp/MyApp.csproj",          // build/run target (optional)
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
| Ctrl+D | Duplicate lines | Ctrl+Shift+K | Delete lines |
| Ctrl+B / Ctrl+J | Toggle side bar / panel | Ctrl+Shift+E / F | Explorer / Search |
| Ctrl+Shift+B | Build | F5 / Shift+F5 | Run / Stop |
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
  - `{"type":"status","id","text","tooltip","command","alignment":"left|right"}`
  - `{"type":"log","text"}`
  - `{"type":"openFile","path","line","column"}`
  - `{"type":"showView","id"}`
- **IDE → module:**
  - `{"type":"activate","workspace"}`
  - `{"type":"executeCommand","id"}`
  - `{"type":"event","name":"folderOpened|fileSaved|themeChanged","path"}`

`samples/hello-module` is a complete Python example: a command with a key binding plus a status bar clock.

## Limitations and next steps

- The Linux build has not been compiled or run in this environment yet (development happened on Windows).
  GPUI's Linux backend is the one Zed ships, so expect it to work, but treat the first Arch build as a test.
- csharp-ls loads the project from the opened folder. Very large solutions take a while before diagnostics
  appear (the status bar shows progress).
- IME composition is basic: committed text is inserted, but there's no inline pre-edit display.
- Git: pull is fast-forward only, there is no merge-conflict editor, no side-by-side diff and no blame yet.
- Not implemented yet: debugger (netcoredbg/DAP), integrated terminal, folding, minimap, multi-cursor and
  split editors. Modules can't add custom UI views yet, only commands, status items and logs.
