<div align="center">

# ByteForge

**A fast, modular, VS Code–style IDE for C#, built from scratch in Rust.**

[![Version](https://img.shields.io/github/v/tag/DevStarByte/ByteForge?label=version&color=blue&sort=semver)](https://github.com/DevStarByte/ByteForge/tags)
[![Rust](.github/assets/rust-badge.svg)](https://www.rust-lang.org/)
[![GPUI](https://img.shields.io/badge/UI-GPUI-084CCF?logo=zedindustries&logoColor=white)](https://github.com/zed-industries/zed)
[![.NET](https://img.shields.io/badge/.NET-SDK-512BD4?logo=dotnet)](https://dotnet.microsoft.com/)
[![Linux](https://img.shields.io/badge/Linux-X11%20%7C%20Wayland-FCC624?logo=linux&logoColor=black)](#install)
[![Windows](https://img.shields.io/badge/Windows-supported-0078D6?logo=windows&logoColor=white)](#install)
[![macOS](https://img.shields.io/badge/macOS-Apple%20Silicon%20%7C%20Intel-000000?logo=apple&logoColor=white)](#install)
[![License: GPL-2.0](https://img.shields.io/badge/license-GPL--2.0-blue)](LICENSE)

</div>

ByteForge is a GPU-rendered IDE for C# and .NET, built on [GPUI](https://github.com/zed-industries/zed), the UI
framework behind Zed. Every part of the window can be restyled and rearranged through theme files that reload
live, and modules written in any language can extend it.

## Features

- **Editor:** multiple cursors, folding, minimap, snippets, find & replace, and tree-sitter highlighting that
  runs in the background (C#, Razor, JSON, XML, YAML, JavaScript / TypeScript, CSS / SCSS, HTML, SQL, Markdown,
  PowerShell and shell scripts); renaming an XML, HTML or Razor tag renames its closing tag
- **Sessions:** reopens the last folder with its open files (split included), each at its caret, in the window's
  last place and size
- **Go Back / Go Forward:** the mouse's back and forward buttons (or Alt+←/→, Ctrl+-/Ctrl+Shift+- on macOS)
  return to where the caret was and forward again, across files
- **C#:** completion, hover, go to definition, find references, rename, quick fixes and inlay hints, powered by
  `csharp-ls` (Roslyn)
- **Code Lens:** reference counts above types and members ("3 references", click to list them) and Run Test /
  Debug Test above tests
- **Call and Type Hierarchy:** who calls a method and what it calls (Shift+Alt+H), base and derived types of a
  type, as a tree in the panel that goes deeper when opened
- **Refactoring:** a Refactor menu (Ctrl+Shift+R, the menu bar or right-click) with Extract Method, Extract Local
  Function, Extract Interface / Base Class, Introduce Variable / Constant / Field / Parameter, Inline, Move Type to
  File and Remove and Sort Usings; what a refactoring names (`NewMethod`, `value`) goes straight to Rename
- **Razor and MSBuild:** highlighting for `.razor`, `.cshtml`, `.csproj` and other XML project files
- **Debugger:** breakpoints, stepping, variables and a Debug Console through netcoredbg; F5 runs ASP.NET Core apps
  with their launch profile (shown in the status bar; click it or use "Select Launch Profile…" to switch); attach
  to a running process, also inside a Docker container (netcoredbg is put into the container for you)
- **EF Core migrations:** add, remove, update the database (to any migration), generate SQL scripts and drop the
  database from the Run menu, with the `DbContext` and migrations found in the source; the EF Core view lists each
  context's migrations, applied or pending
- **Hot reload:** "Run with Watch" (Alt+F5) runs the app under `dotnet watch`, applies edits while it runs and
  restarts by itself when an edit can't be applied
- **Templates:** New Project from any `dotnet new` template (added to the solution), New Item with C# class,
  interface, record, enum and struct (namespace from the project and folder) plus the SDK's item templates
- **User secrets:** "Manage User Secrets" opens the project's `secrets.json`, setting it up first if needed
- **Tests:** a Test Explorer for xUnit, NUnit and MSTest, with run and debug for any test
- **Benchmarks:** Run Benchmark above BenchmarkDotNet methods (Release, just that one), results in a Benchmarks tab
  with the change since the last run
- **Live metrics:** CPU, memory, GC, thread pool, exceptions and requests of a running .NET app as sparklines
  (through `dotnet-counters`), and the app's CPU and memory in the status bar while it runs
- **Tasks:** `.byteforge/tasks.json`, package.json scripts, Makefile targets and justfile recipes in a Tasks view
  and "Run Task…", each in a terminal
- **Windows Forms designer:** View Designer (Shift+F7, or the button on a form's tab) shows the form of a
  `Form1.cs` / `Form1.Designer.cs` with a toolbox, a property grid and the component tray. Place, move and resize
  controls with the mouse or the arrow keys, drop them into panels, group boxes, tab pages and split containers,
  set properties and hook events (double-click a control: its handler in the code); snap lines line them up
  while dragging, Align and Same Size line up several, and Ctrl+C / Ctrl+X / Ctrl+V copy controls within a
  form or into another one. Everything is written into
  `InitializeComponent` in the file's own style (.NET Framework or .NET 6+), so the designer file saves, undoes
  and diffs like any file; it works on Linux and macOS too, as ByteForge draws the form itself
- **Resource editor:** `.resx` files as one table per resource, a column per language: edit in place, add and
  remove keys everywhere at once, missing translations marked and in Problems
- **Solution and NuGet:** a Solution view with the projects of the `.sln`/`.slnx`, their packages and project
  references; search, add, update and remove NuGet packages, and see which ones have updates; Central Package
  Management (`Directory.Packages.props`): turn it on, see each version and who uses it, move stray versions in
- **Git:** a Source Control view, change markers in the gutter, a diff editor (side by side or inline, syntax
  highlighted, changed words marked, F7 / Shift+F7 between changes, the working tree editable in place), blame of the current line, file history,
  merge-conflict resolution (accept current / incoming / both), branches, and pull (merge, or rebase with
  `pull.rebase`) / push / fetch. The view's "…" menu
  (also in the command palette) has Undo Last Commit, Commit (Amend), Revert Commit, the stash (stash, pop, apply,
  drop), Discard All, Merge Branch / Abort Merge, Rename Branch and tags; right-click a commit in HISTORY to tag,
  revert or copy it
- **Docker:** containers, images, volumes and Compose, plus Dockerfile generation for .NET projects (the Docker
  module, installed from the Modules view)
- **Terminal:** your own shell on a real PTY, with true color and several tabs
- **Formatting:** ReSharper's formatter (the one Rider uses) by default, or Roslyn; CSharpier from the Modules view

## Install

### Arch Linux

Add the DevStarByte repo to `/etc/pacman.conf` (one repo for all DevStarByte projects):

```ini
[devstarbyte]
SigLevel = Required
Server = https://raw.githubusercontent.com/DevStarByte/arch-repo/master/$arch
```

and trust its signing key once:

```sh
curl -sL https://raw.githubusercontent.com/DevStarByte/arch-repo/master/devstarbyte.asc | sudo pacman-key --add -
sudo pacman-key --lsign-key 2F59DCCF36A08B6C4BB5556DF8E225736ED216F0
```

Then install it, along with the C# language server:

```sh
sudo pacman -Sy byteforge
dotnet tool install -g csharp-ls
```

### Windows

Download `ByteForge-<version>-windows-x64-setup.exe` (installer, no admin rights needed) or the portable
`.zip` from [Releases](https://github.com/DevStarByte/ByteForge/releases). You also need the
[.NET SDK](https://dotnet.microsoft.com/download) and the C# language server:

```sh
dotnet tool install -g csharp-ls
```

Windows builds aren't code-signed, so Windows SmartScreen warns the first time you run the installer or
`byteforge.exe`: choose "More info" → "Run anyway".

### macOS

Download `ByteForge-<version>-macos-arm64.dmg` (Apple Silicon) or `ByteForge-<version>-macos-x64.dmg` (Intel) from
[Releases](https://github.com/DevStarByte/ByteForge/releases) and drag ByteForge into Applications. The app
isn't notarized yet, so the first time right-click it and pick **Open** (or run
`xattr -dr com.apple.quarantine /Applications/ByteForge.app`). You also need the
[.NET SDK](https://dotnet.microsoft.com/download) and the C# language server:

```sh
dotnet tool install -g csharp-ls
```

Shortcuts use ⌘ where Windows and Linux use Ctrl (⌘⇧P for the command palette, ⌘S to save, …), the menus
live in the macOS menu bar, ⌘+Click goes to a definition, and the Explorer shows Finder's file and folder
icons (`"explorer.nativeIcons": false` brings back the theme's). ByteForge picks up the `PATH` of your login
shell, so tools installed through Homebrew or `~/.dotnet/tools` are found even when it's started from Finder.

### Updates

On Windows (installed with the setup) and macOS, ByteForge checks for a new release every time it starts and offers to
install it: it downloads the update, closes, installs and starts again. "Check for Updates…" (Help menu, or the
app menu on macOS) checks right away. Turn the check off with `"update.checkAutomatically": false`, or get
betas too with `"update.includePrereleases": true`. The portable zip and Linux packages only point you to the
new release.

Pre-releases (betas) are marked as such on the Releases page; each release also carries the Arch package
(`sudo pacman -U byteforge-*.pkg.tar.zst`) for trying a beta without the pacman repo.

### From source

You need Rust 1.85+, the .NET SDK and `csharp-ls`:

```sh
dotnet tool install -g csharp-ls
cargo run --release -p byteforge -- ~/code/my-project
```

`./packaging/linux/install.sh` installs a build to `~/.local`, `packaging/arch` has a PKGBUILD
(`makepkg -si`), and on macOS `packaging/macos/bundle.sh` builds `dist/ByteForge.app` and a `.dmg`.

**Optional:**
- Debugging uses [netcoredbg](https://github.com/Samsung/netcoredbg) (AUR: `netcoredbg`). When none is installed, ByteForge
  downloads the release for your system (Windows, macOS, Linux x64/arm64) into its cache on the first F5.
- For ASP.NET Core and Blazor projects, install the ASP.NET Core runtime and targeting pack.

> [!NOTE]
> On Linux, GPUI renders through Vulkan, so you need the Vulkan driver for your GPU (`vulkan-radeon`,
> `vulkan-intel` or `nvidia-utils`).

Start it from a terminal with `byteforge .` (or a folder or file path). On Linux and macOS it detaches: the
prompt comes back right away, and closing the terminal leaves ByteForge running. `byteforge --foreground`
stays attached and shows the log output. `BYTEFORGE_CONFIG_DIR=<folder>` starts it with its own settings,
themes and modules in that folder (to try a theme or a build without touching your setup, or as a portable install).

## Customize

- **Themes:** 11 built-in themes. Use **Preferences → Customize Current Theme** to make your own; it reloads
  live when you save. See [docs/themes.md](docs/themes.md).
- **Settings and shortcuts:** `~/.config/byteforge/settings.json`. See [docs/settings.md](docs/settings.md).
- **Modules:** install them from the Modules view, or write your own in any language. See
  [docs/modules.md](docs/modules.md).

## Docs

| Page | Contents |
|---|---|
| [Themes and layout](docs/themes.md) | theme files, every theme section, rearranging the window |
| [Settings](docs/settings.md) | settings reference and default shortcuts |
| [Modules](docs/modules.md) | the module registry, writing a module, the protocol |
| [Architecture](docs/architecture.md) | crate layout and performance notes |
| [Roadmap](ROADMAP.md) | what's planned |

## Known limitations

- Razor files (`.razor`, `.cshtml`) get completion, hover, go to definition and live errors only with the Roslyn
  server (`"csharp.server": "roslyn"`); with csharp-ls their errors show up after a build. HTML in them has no
  completion or hover of its own (there is no HTML language server).
- No Change Signature refactoring: neither csharp-ls nor the Roslyn server offers it outside Visual Studio.
- Apps in Alpine (musl) containers can't be debugged (netcoredbg has no musl release). Blazor WebAssembly apps
  are debugged in Edge, Chrome or Chromium (Firefox isn't supported); a Blazor Web App's server project is
  debugged as usual, its WebAssembly client part not yet.
- The Windows Forms designer draws controls itself, so they look close to, not exactly like, Windows Forms at run
  time (images, custom controls and the cells of table layouts are approximate). It edits menu and tool strip
  items, grid columns and data bindings as code only, and keeps localized forms' `resources.ApplyResources` as
  written without editing their translations.
- Windows downloads aren't code-signed yet and the macOS app isn't notarized (see [Install](#install)).
- Very large solutions take a while before diagnostics appear.

## License

ByteForge is licensed under the [GNU General Public License v2.0](LICENSE).
