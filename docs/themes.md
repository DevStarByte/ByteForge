# Themes

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

## Theme sections

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
- **solution**: `solution.updateForeground` for packages with a newer version (in `colors`).
- **minimap and groups**: `minimap.background`, `minimap.selectionHighlight`, `minimap.caretLine`,
  `minimapSlider.background` / `hoverBackground` and `editorGroup.border` (in `colors`), `minimap.width` (in
  `metrics`).
- **git colors** (in `colors`): `gitDecoration.*ResourceForeground`, `editorGutter.added/modified/deletedBackground`,
  `diffEditor.inserted/removedLineBackground`, `activityBarBadge.*`, `badge.*`; sizes in `metrics`:
  `editorGutter.changeWidth/changeOffset/changeRadius`, `scm.iconSize`, `activityBarBadge.size`.
- **effects**: window backdrop (`blur`, `transparent` or `none`) and shadows for the palette and other widgets.
- **layout**: where every part of the window goes. Each container is an ordered list of parts: reorder
  parts to move them, leave them out to hide them (see [Layout](#layout) below).
- **icons**: replace any built-in icon (logo, files, search, window buttons, …) with SVG path data. Every file
  type has its own icon (a page with the type's symbol: `#` for C#, `{ }` for JSON, `< >` for markup, …), colored
  by `icon.csharp`, `icon.json`, `icon.razor`, `icon.yaml`, `icon.script` and the other `icon.*` keys. Modules can
  add icons for more file types (the preinstalled Docker module does this for Docker files).

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

