# Changelog

What changed in each version of ByteForge. Versions before 1.0 are betas, published as pre-releases
on the [Releases](https://github.com/DevStarByte/ByteForge/releases) page.

## 1.9.1

- A setting changed in ByteForge could briefly snap back: when the settings file was reloaded while a new
  value was being written, the reload put back what the file said before. The write and the reload now take
  turns.

## 1.9.0

- An Avalonia designer for `.axaml` views: View Designer (Shift+F7, or the layers button on the tab) shows the view
  laid out the way Avalonia does it (Grid rows and columns with Auto, star and pixel sizes, StackPanel, DockPanel,
  WrapPanel, UniformGrid, Canvas, Border, ScrollViewer, TabControl, Expander, …), its controls in the Fluent
  theme's look, light or dark (as the view or App.axaml asks, or switched in the toolbar), with zoom. The toolbox
  places controls by click or drag: into a Grid cell (`Grid.Row` / `Grid.Column`), between the children of a
  StackPanel (a line shows where), on a side of a DockPanel, at a point of a Canvas, or into an empty Border or
  Button. Drag a control to move it (between panels too: its old panel's attached properties go), pull its handles
  to resize it, nudge it with the arrow keys (pixels on a Canvas, cells in a Grid, places in a StackPanel), Delete,
  Ctrl+C / Ctrl+X / Ctrl+V as XAML. The Outline tab lists every element (also the ones not shown); the property grid
  has the control's properties by category, the attached ones of its panel and every other attribute as text; the
  Events tab hooks up handlers (double-click a control: `Button_OnClick` in the `.axaml.cs`, its `using` added).
  Every change is a small edit of the XAML: comments, formatting and attribute order stay. The view and the code
  stay in step as you type; while the XAML has an error the designer keeps the last good view, and the code's caret
  selects the element it is on. F7 goes back to the code at the selected element.

## 1.8.0

- Pick the project to start. In a folder with several projects, **Run › Select Startup Project…** (also in the
  command palette) lists them, apps first, and the one you pick is what F5, Ctrl+F5 and Run with Watch start from
  then on. ByteForge remembers it per folder; when the project is gone it falls back to the first app again. The
  startup project shows in the status bar (click it to change it) and is marked with ▶ in the Solution view, where
  "Set as Startup Project" sits among a project's hover buttons. Themes can place it with the `startupProject`
  status and title bar part (Rider Dark shows it beside the launch profile). The `dotnet.project` setting still
  works as the default.
- The launch profile in the status bar now updates as soon as a folder is opened.

## 1.7.3

- Four new themes. **Paper**: ink on warm paper with a serif interface and a single red accent. **Abyss**: deep-sea
  navy with a bioluminescent glow. **High Contrast**: black and white with solid borders, a yellow focus and a
  steady, wider caret, for readability. **Zen**: just the editor: no activity bar or menu, the side bar hidden at
  start, a quiet status bar and wide margins around the text.

## 1.7.2

- The settings screen, tidied up. Each setting takes one line less: its settings.json key sits beside its name, the
  description below, and the switch or value right next to them instead of at the far edge of the window. A changed
  setting is marked like a changed line in the editor, by a bar at its edge, and goes back to the default with the
  undo button beside it (its tooltip says what the default is). The column on the left counts the changed settings
  of each section, so you see at once where your configuration differs. The title and search stay at the top while
  the list scrolls. Theme keys: `settings.modifiedIndicator`, `settings.rowBorder`.

## 1.7.1

- The AI assistant, redesigned. The conversation reads like notes beside the code: your messages carry a rule in the
  theme's accent and the file and line they were about (click it to go there); answers flow as plain text under the
  model's name, with the reasoning behind a "Show reasoning" toggle and the tools it used as one compact line. A
  proposed change is drawn like code: the file, how many lines go and come, and the diff in the editor's font and
  syntax colors with the file's own line numbers (common indentation taken away, long lines scroll), then Accept
  and Reject. The input sits in a frame with the file it will be about (✕ leaves it out) and a round send button;
  an empty chat offers three ways to start. The model's name heads the view, the tokens used are in its tooltip.
  New theme keys under `assistant.*` (each falls back to an existing color).
- Settings → AI Assistant suggests the models to pick, for the provider in use: with `anthropic` the current Claude
  models (Opus 5.5, Sonnet 5.5, Haiku 5.5, Fable 5.2; hover one for when to choose it), with `local` the models the
  local server has (asked from Ollama or LM Studio; Refresh after `ollama pull`). A click picks one. Only the chosen
  provider's settings are listed: the Claude model, effort and key, or the local server, model and key.

## 1.7.0

- AI Assistant: a new side bar view (the sparkle in the activity bar, Ctrl+Alt+I, View → AI Assistant). Chat with
  **Claude** through the Anthropic API (`claude-opus-5-5` by default, `ai.model`; your key from `ANTHROPIC_API_KEY`
  or `ai.apiKey`), or with a **model on your own machine** through Ollama, LM Studio or llama.cpp's server (`ai.provider`:
  `local`, `ai.local.url`, `ai.local.model`; `qwen3.5` by default). The assistant reads, lists and searches the
  workspace itself; every change it wants to make, an edit or a new file, shows as a diff with **Accept** and
  **Reject**: accepted edits go into the open editor (unsaved, one undo step) or straight into the file (its line
  endings and BOM kept), new files open. The active file and selection go along with each message (click the line
  under the input to turn that off). Ctrl+Enter sends, Stop stops an answer, New Chat starts over. Claude's
  reasoning summary can be opened above its answer; a refusal or a cut-off answer is said plainly. With the
  `anthropic` provider your messages and the code the assistant reads go to Anthropic (see the privacy statement);
  with `local` they stay on your machine.

## 1.6.0

- Profiler: a new PROFILER panel tab (Test → Profiler, Start or Stop Recording a CPU Profile, Take Memory
  Snapshot). Pick a .NET process on the left (the app you run or debug is picked by itself), press Record CPU
  Profile, do the slow thing, press Stop. `dotnet-trace` samples the threads meanwhile; the result shows as
  **Hot Methods** (time in the method itself and in everything it called), a **Call Tree** opened along the
  hottest path, and a **Flame Graph** (click a frame to zoom in, a row above to zoom out). Count only time
  running .NET code, or also time waiting; Only my code hides the framework. Clicking a method opens its
  source. **Memory snapshots** (`dotnet-gcdump`) list the types on the heap by size and count; a second
  snapshot of the same process shows what grew since (a leak keeps growing). ByteForge offers to install
  `dotnet-trace` and `dotnet-gcdump` when they're missing. The trace file stays in the temp folder for other
  tools (its path is in the Output). Theme keys: `profiler.flame.userCode`, `profiler.flame.otherCode`,
  `profiler.flame.foreground`, `profiler.barBackground`, `profiler.grew`, `profiler.shrank`.

## 1.5.0

- Code coverage in the gutter: "Run All Tests with Coverage" and "Run Test at Cursor with Coverage" (Test menu, and
  a button in the Testing view) run the tests collecting coverage, with coverlet when the test project references
  `coverlet.collector` (the xunit, NUnit and MSTest templates do), else with Microsoft's collector, which comes with
  Microsoft.NET.Test.Sdk, so nothing needs installing. Each line the run measured gets a bar between the line
  numbers and the folding arrows: covered, partly covered (a branch not taken) or not covered. Bars move with
  their lines; a line you edit loses its bar. The Output shows the line and branch coverage of the code under test
  (test projects aren't counted) and the least covered files. "Show or Hide Coverage" and "Clear Coverage". Theme
  keys: `editorGutter.coveredBackground`, `partiallyCoveredBackground`, `uncoveredBackground`, `coverageWidth`,
  `coverageGap`.

## 1.4.2

- The hover popup no longer scrolls: it takes the room there is above or below the line (no longer at most
  360 pixels), and the mouse wheel over it does nothing. Its text can now be selected with the mouse (drag, or
  double-click for a whole block) and copied with Ctrl+C; without a selection in it, Ctrl+C copies the
  editor's selection as before.
- Many open tabs no longer squeeze each other until their names run into one another and the close buttons are
  gone: tabs keep their width and the tab strip scrolls (the mouse wheel scrolls it sideways), a tab that's
  activated is scrolled into view, and the buttons at the strip's right end (designer, preview) stay in place.

## 1.4.1

- Keyboard Shortcuts on macOS: keys recorded there (gpui writes them as `alt-cmd-b`) are now compared with the
  built-in ones (`cmd-alt-b`) regardless of the order of their modifiers, so a clash with another command is
  shown and recording a command's default keys again leaves no entry in settings.json. Keys are saved with
  their modifiers in one order (ctrl, cmd, alt, shift).
  This also fixes the macOS tests that held back the 1.4.0 release.

## 1.4.0

- Modules in C#: ByteForge runs .NET modules (`"runtime": "dotnet"` in module.json) in one shared .NET module
  host, each in its own load context. Turning a module off, updating or uninstalling it unloads it without
  touching the others; a module that fails stops alone and its error goes to the Output; the host keeps no
  module file open, so an uninstall on Windows never finds them locked. Modules are written against the module
  SDK on the packages branch (docs/modules.md, "Writing a module"); they need .NET 10 or newer. Every module is
  now .NET, and ByteForge no longer runs Python modules: at start it replaces installed ones with their .NET
  versions from the registry, so Python isn't needed any more (on Windows that also ends the trouble with the
  Microsoft Store's python3 and with PATHEXT).
- The GUID Generator module is gone from the registry.
- The ReSharper Formatter is no longer bundled with ByteForge: it's installed from the registry on the first
  start (once; uninstalled, it stays uninstalled).
- A module's themes belong to it: a module writes them into its own `themes/` folder, ByteForge lists them
  while the module is installed and turned on, and drops them with it. Uninstalling the Windows Theme left the
  "Windows" theme behind (and in use); now the theme goes too, and ByteForge switches to ByteForge Dark (or
  Light). The new theme modules remove the copy their old versions wrote into the general themes folder.
- Keyboard Shortcuts (Preferences → Keyboard Shortcuts, also from the Settings screen): every command with its
  keys, searchable by name or by keys ("ctrl+s"), with a filter for the changed ones. Click a command's keys
  and press the new ones (two presses make a chord such as Ctrl+K Ctrl+S; Enter saves, Escape cancels);
  Remove takes them away, Reset brings the default back, and keys another command has too are pointed out.
  It writes `"keybindings"` in settings.json (one entry per line now) and every change takes effect at once:
  the old keys stop working too, which a change in settings.json didn't do before. Menus show the changed keys.
- The Run menu was taller than the screen; it's now four menus, as in Visual Studio: Run (start, run, watch,
  stop, launch profile, user secrets, tasks), Debug (stepping, breakpoints, attach), Build (build, test, clean,
  restore, reload C#, EF Core migrations) and Test (tests, benchmarks, live metrics). A theme that lists its
  menus (`"menuBar.menus"`) with "Run" but none of the new ones gets them after Run.

## 1.3.1

- Windows: modules no longer find their own script when they look for a program of the same name. Windows
  searches a program's current folder first, a module runs in its own folder, and PATHEXT often lists `.PY`,
  so CSharpier found `csharpier.py` instead of `csharpier.exe` and failed with "not a valid Win32
  application" (WinError 193); Aspire, Docker and Lua could do the same. Modules now get programs from PATH
  only.

## 1.3.0

- Sticky scroll: the first lines of the blocks the top of the editor is in (namespace, class, method,
  `#region`, an XML element; not comment runs or `using` blocks) stay pinned at the top while their block
  scrolls by, with their line numbers and colors, and a block's last line pushes its pinned line out. A click
  on a pinned line goes there. The caret moving up stays below the pinned lines. At most 5
  (`"editor.stickyScroll.maxLineCount"`) and no more than a third of the editor; `"editor.stickyScroll.enabled"`
  turns it off (also in Settings). Themes can color it (`editorStickyScroll.*`).
- The Windows Forms designer: "View Designer" (Shift+F7, or the button on the tab of a form's code) opens a form
  or user control from its `.Designer.cs`. A toolbox (Button, TextBox, Label, CheckBox, ComboBox, ListBox,
  DataGridView, Panel, GroupBox, TabControl, SplitContainer, FlowLayoutPanel, TableLayoutPanel, MenuStrip,
  StatusStrip, Timer, dialogs and more) places controls by click, drag or double-click; controls are moved and
  resized with the mouse or the arrow keys and dropped into containers, which follows docking and anchors; the
  property grid edits properties by category (switches, choices, colors, anchors, fonts, list items, and any
  other property as code) and events (double-click: the handler is added to the code file and opened); a
  component tray holds timers, tooltips and dialogs. Changes go into `InitializeComponent` in the file's own
  style, through the file's editor, so undo, save and autosave work as for any file and a save without changes
  changes nothing. ByteForge draws the form itself, so the designer works on Linux and macOS too.
- Designer: copy, cut and paste controls (Ctrl+C / Ctrl+X / Ctrl+V, also in the Edit menu), with what is in
  them (a group box's controls, a menu's items, a list's items) and in the same form or another one: each keeps
  its name when it is free there, else gets the next one (`button2`); a copy pasted where the original is moves
  10 px down and right; it goes into the selected container, next to the selected control, or onto the form,
  in that file's code style; event handlers stay hooked up within the same form. Ctrl+A selects every control.
- Designer: localized forms. A form whose code applies resources (`resources.ApplyResources(button1,
  "button1")`, Visual Studio's Localizable forms) shows its controls with the values of `Form1.resx`: their
  text, place and size, anchors and more. The toolbar's Language buttons show the form in one of its languages
  (`Form1.de.resx`'s values over the neutral ones). Moving, resizing and editing a property the resources
  hold change the file of the language shown (a language without a file gets one), not the code; the `.resx`
  files are saved with the designer (the tab shows them unsaved until then), and Ctrl+Z / Ctrl+Y undo and
  redo them with the code. Copying a localized control into another form takes its values along as code;
  deleting one removes its entries from every language's file.
- Designer: menus and grid columns are edited on the form. Clicking a menu item selects it and opens its menu
  (submenus open to the right); "Type Here" at the end of a menu, tool or status strip and of each menu adds an
  item, and the text typed for it names it as Visual Studio does (`&File` → `fileToolStripMenuItem`). Items
  move with Move Up / Down (Left / Right on a strip) and go with Delete, which keeps the others in their list.
  A grid's column headers are drawn at their widths and clicked to select a column; "+" adds a text column and
  the toolbar's Add Column the other kinds. The property grid shows an item's or column's own properties
  (Checked, ShortcutKeys, ToolTipText; HeaderText, Width, ReadOnly, AutoSizeMode, …), not a control's.
  Fixed on the way: a component renamed in the grid stays selected, and after Enter in a grid field the
  designer has the keyboard again (Ctrl+S, Delete and the arrows work without a click).
- Designer: snap lines. A control dragged or resized snaps to the edges of the controls next to it, to the
  recommended 6 px between controls and to 12 px from the form's edge (3 in a container), and the lines show
  while dragging (`formDesigner.snapLine` / `marginLine`); Alt drags without snapping. With several controls
  selected, the toolbar's Align (Lefts, Centers, Rights, Tops, Middles, Bottoms) and Same Size line them up
  with the primary one (the one clicked first).
- Go to File (Ctrl+P) takes a line, as in VS Code: `Form1.cs:42` or `Form1:42:7` opens the file at that line
  (and column).
- Source Control: with nothing to commit, the main button says what's next: "Push 1 commit", "Pull 2
  commits", "Sync Changes ↓2 ↑1" or "Publish Branch". A failed push, pull or commit shows its message in the
  view (with Show Output) until dismissed, and a successful one confirms what it did. HISTORY marks the
  commits not pushed yet, lists incoming ones (click to pull), and gives the messages the room: a branch and its
  remote at the same commit share one badge ("master · origin"), `origin/HEAD` is left out, tags are outlined.
- Typing is smoother, more so in longer files. The caret's blinking redrew the whole window about 30 times a
  second all the time (with the default "smooth" blinking): also while typing, when the editor had no focus and
  with the window in the background. It now redraws only while the caret shows and blinks. Fold markers and
  the language server's colors are worked out on a background thread instead of while drawing, and the
  minimap is rebuilt only when what it shows changed.
- Source Control: HISTORY draws the commit graph. Each branch runs in its own colored lane, merges and branch-offs
  curve between lanes, and the checked-out commit is a ring. The remote branch's commits show too, so a branch
  and its diverged remote sit side by side; lines from commits not pushed yet (or not pulled yet) are dashed.
  Lanes go on through a commit's expanded files. Colors: `scmGraph.foreground1` … `foreground5`.

## 1.2.0

Section 6 of the roadmap: the rest of a .NET project. Released as a beta first: none of it has been clicked
through by hand yet.

- Highlighting for JavaScript, TypeScript, CSS, SCSS, HTML (with its `<script>` and `<style>`), SQL, Markdown,
  PowerShell and shell scripts. SQL uses a small built-in lexer (comments across lines, strings, quoted names,
  T-SQL `@variables`, PostgreSQL `$$` bodies) rather than a grammar: the SQL grammar's 47 MB of C didn't
  build on Windows, which stopped 1.2.0-beta.1.
- Linked editing: renaming an XML, HTML or Razor tag renames its closing tag ("editor.linkedEditing").
- Tasks, built in: a Tasks view and "Tasks: Run Task…" / "Rerun Last Task" for `.byteforge/tasks.json`,
  package.json scripts, Makefile targets and justfile recipes, each run in a terminal; "Configure Tasks" writes
  an example file.
- The EF Core view: each project's `DbContext`s and their migrations, applied or pending, with Update Database
  to Here, Script from Here and Remove, and Add Migration, Update, Drop, Scaffold and Refresh on top.
- Benchmarks: "Run Benchmark" above BenchmarkDotNet classes and methods (and Run Benchmark at Cursor) runs just
  that one in Release; a Benchmarks panel tab shows mean, error and allocated, and the change since the last run.
- Live metrics: a Metrics panel tab with the running .NET processes and, for the one picked, CPU, working set, GC
  heap, GC collections, thread pool queue, exceptions and requests per second as sparklines, from
  `dotnet-counters` (offered for install when missing). The app you run or debug is watched by itself and its
  CPU and memory show in the status bar ("metrics.statusBar" turns that off).
- The resource editor: a `.resx` file opens as a table of its resource in every language. Values are edited in
  place, keys added and removed in every language at once, languages added, missing translations marked and
  filterable; saving touches only the entries that changed. Problems lists keys a language lacks.
- Central Package Management: the Solution view's NuGet node shows each version in `Directory.Packages.props`
  and the projects using it, lists references that still carry their own version with "Move to
  Directory.Packages.props", and "Enable Central Package Management" sets it up from the projects' versions.

## 1.1.4

Found by using ByteForge on a real solution (Aspire, an API and a Blazor frontend, `.slnx`).

- After a restore (Restore, Build, Run, Test or Watch), the C# language server restarts by itself when the
  packages changed, so their APIs resolve. Before, a fresh clone kept its package errors ("does not contain a
  definition for 'AddOpenApi'") after the first build, until "Restart C# Language Server". Builds that restore
  nothing new don't restart it.
- The Solution view lists solution items: files a solution folder holds (`<File>` in a `.slnx`,
  `SolutionItems` in a `.sln`), under their folder, opened with a click. A folder with only files says "3 files"
  instead of "0 projects".

## 1.1.3

- Windows: updating a ByteForge installed for all users (in Program Files) works. The update's setup ran
  without admin rights and failed with "access denied"; now it asks for them once (a UAC prompt), installs,
  and starts ByteForge again as you. Updating out of an older version gets this too, since the new version's
  setup does the installing.

## 1.1.2

1.1.1 wasn't released (a test caught the diff bug below); 1.1.2 has its change too.

- Linux: ByteForge has its app icon in the app launcher, the taskbar and the window switcher. The pacman packages,
  `packaging/arch` and `install.sh` install it into the hicolor icon theme (SVG plus PNGs from 16 to 512 px),
  and the desktop entry points to it.
- The diff editor keeps showing unsaved edits of the file: when git's status refreshed (after a commit, a
  checkout, or git touching its index), the diff went back to the file on disk and the unsaved change vanished
  from it, along with its Stage and Revert buttons.

## 1.1.0

Released as betas while section 1 of the roadmap (everyday use) is finished; the next full release follows.

- Razor IntelliSense with the Roslyn server: in `.razor` and `.cshtml` files, completion, hover, go to definition
  and errors as you type (no build needed), for the C# in them. The Roslyn server now comes from nuget.org
  (Microsoft's `roslyn-language-server` package for your platform, signature still checked), which carries the
  Razor extension; run "C#: Install or Update Roslyn Language Server" once to get it. HTML in Razor files has no
  completion of its own yet.
- The Roslyn server runs with its telemetry off.
- Blazor WebAssembly debugging: F5 on a standalone Blazor WebAssembly app builds and runs it, opens it in Edge,
  Chrome or Chromium (`"debug.browser"` picks one) and stops at breakpoints in `.razor` and `.cs` files, with
  stepping, the call stack, locals, watch and hover values, and the app's Console output in the Debug Console.
  Works with .NET 11's new gateway too. Shift+F5 (or closing the browser) ends it and stops the app.
- Markdown preview: it can be used now, not only read. It scrolls with the wheel and the keyboard (arrows, Page
  Up/Down, Home/End, Space), text can be selected with the mouse and copied (Ctrl+C, Ctrl+A, double click for a
  block), links open (and `#heading` links jump to the heading), and task list checkboxes tick or clear the
  source. The preview keeps its scroll position when you switch tabs.
- "Install or Update Roslyn Language Server" no longer fails with "directory not empty" when an older version is
  still in use, and never goes back to an older version than the one installed.
- Windows: stopping a debug session also ends what the debugger started (the app, a browser), not only the
  debugger.
- Git pull isn't fast-forward only any more. When your branch and the remote both have new commits, Pull (and
  Sync) merges them, or rebases yours on top when `pull.rebase` (or `branch.<name>.rebase`) is set; `pull.ff =
  only` is still honored. A pull that stops at conflicting changes says what to do next: resolve them in the
  Merge Editor, then commit (merge) or Commit / "Git: Continue Rebase" (rebase, each commit keeps its message),
  or "Abort Merge" / "Abort Rebase" (new, in the Source Control "…" menu and the command palette).
- Problems lists the build's errors in C# files that aren't open. With the C# language server running they were
  hidden, as if the server reported them, but it only reports open files; so a failed build (also before F5)
  showed an empty Problems tab. An error the editor already shows is listed once.
- Code Lens: a row above types and members says how often they're referenced ("3 references"); click it to
  list the references. Tests get "Run Test | Debug Test" (a test class "Run All Tests | Debug All Tests"). The
  row sits above the attributes, like Rider's. The counts the screen shows are asked for first, and are asked for
  again after edits, after saving a C# file and once the server has loaded the solution. Off with
  `"editor.codeLens": false` (also on the settings screen); themes color it with `editorCodeLens.foreground`.
- Call Hierarchy (Shift+Alt+H) and Type Hierarchy (Go menu, or right-click in the editor): who calls a method,
  property or constructor and what it calls, or a type's base types and derived types, as a tree in the panel's
  new Hierarchy tab. Branches are asked for from the language server when opened; Callers / Callees (Base Types /
  Derived Types) switch the direction. A click opens a caller at its call, anything else at its declaration.
- README: the known limitations match the app again.

## 1.0.4

- Terminal: Space types a space again (Windows delivered it as a named key without a character, so it was dropped).

## 1.0.3

- Windows: no more console windows popping up. Every program ByteForge starts runs without one, also the
  few that didn't yet (the .NET runtime check at start, module downloads, opening links in the browser,
  stopping a run, the debugger and Roslyn downloads), and a test keeps it that way.
- The C# status shows "ready" again with csharp-ls 0.29, which ends loading with "Loaded app" instead of
  "Finished …". ByteForge now goes by the protocol (every progress has ended), not by the wording.
- The downloads carry only the preinstalled module, the ReSharper Formatter. Aspire, Azure and SQL were
  still bundled (unused); like every other module they come from the Modules view. Aspire 0.2.0 and
  SQL 0.3.0, which list their settings on the settings screen, are published there.

## 1.0.2

- The Windows installer (`ByteForge-<version>-windows-x64-setup.exe`) is on the release again. It was
  built but never attached, so 1.0.0 and 1.0.1 have only the portable `.zip`, and installed copies couldn't
  update themselves.
- README: Windows builds aren't code-signed; how to get past the SmartScreen warning.

## 1.0.1

Not released: its build failed in the tests. Its changes came out with 1.0.2.

- Settings screen: the search box spans the page and shows its "Search settings" hint again. Before, it
  was a narrow sliver with only the caret visible. Text boxes now fill their box wherever they are placed.
- Faster start with the Roslyn server (`"csharp.server": "roslyn"`): its checks (installed version, .NET
  runtime, the solution to open) run in the background. Before, the window froze for several seconds
  right after it opened.

## 1.0.0

The first stable release: everything on the roadmap is in, tested headlessly and by hand on the real app.
What ByteForge does is in the [README](README.md#features).

Known limits:
- Razor and Blazor files (`.razor`, `.cshtml`) are highlighted, but have no IntelliSense with the Roslyn
  language server (`"csharp.server": "roslyn"`): it needs the Razor extension VS Code's C# extension loads.
- No Change Signature refactoring: neither csharp-ls nor the Roslyn server offers it outside Visual Studio.
- No debugging inside Alpine (musl) containers: netcoredbg has no musl release.
- Windows downloads aren't signed yet (SmartScreen warns once) until the SignPath Foundation approves the
  project; the macOS app isn't notarized (right-click → Open the first time).

Changes since 0.10.2:

- CI: clippy and the whole test suite on Arch Linux (in an archlinux container), Windows and macOS for every push and pull request.
- The privacy statement lists the update check, the netcoredbg download and the Roslyn server download.
- Small code cleanups (no clippy warnings).
- Source Control: the commit message box and the Commit button always span the side bar; the branch name
  stays visible in a narrow side bar (the sync note shortens first).
- Text boxes scroll sideways to keep the caret in view, and long text no longer runs out of the box.
- The commit message box takes several lines, like VS Code's: the text wraps, the box grows (up to 10 rows),
  Enter starts a new line and Ctrl+Enter (Cmd+Enter on macOS) commits. Undo Last Commit puts the whole
  message back, body included.

## 0.10.2 (beta 1)

- The update check runs a few seconds after every start. Before, it only ran once files had changed in
  the workspace (at most once a day). `"update.checkAutomatically": false` still turns it off.

## 0.10.1

- No compiler warnings on any platform.

## 0.10.0

- The ReSharper Formatter (`jb cleanupcode`, the formatter of Rider and ReSharper) is now the one
  preinstalled module, instead of CSharpier. CSharpier stays in the module registry; installs that
  have it keep it.

## 0.9.0

- Rider Dark theme: main toolbar with project badge, branch and run configuration, an icon-only tool
  window stripe, flat tabs, the file's path in the status bar, Rider's syntax colors.
- New layout parts any theme can use: project, launch profile, run and debug in the title bar; the
  path in the status bar; side bar titles without capitals.
- `BYTEFORGE_CONFIG_DIR` starts ByteForge with its own config folder.

## 0.8.1

- Updating a module stops it first and restarts it afterwards; a failed update restarts the old
  version, and the module folder is never left half deleted.

## 0.8.0

- Git: a "…" menu in Source Control with Commit (Amend), Undo Last Commit, Revert Commit, Discard All,
  the stash (stash, pop, apply, drop), Rename Branch, Merge Branch, Abort Merge, tags. Right-click a
  commit in HISTORY to copy, tag, revert or undo it. Risky actions ask first.
- Language servers and git answer in English on non-English systems.
- Hovers decode HTML entities; the Output panel scrolls to the newest line when shown.

## 0.7.0

- Settings: a group list on the left of the settings page; modules can declare their own settings.
- Windows: the app icon in the taskbar and title bar; refactorings and test runs work on non-English
  systems; modules that need `python3` find a real Python.

## 0.6.1

- Windows: moving files to the Recycle Bin no longer crashes the app.

## 0.6.0 and earlier

See the [git history](https://github.com/DevStarByte/ByteForge/commits/master).
