# ByteForge Module Registry

This is the `packages` branch of ByteForge. It holds the module registry that
the IDE's Modules view reads, kept apart from the source on `master`.

## Install ByteForge on Arch Linux

The pacman package moved to the shared DevStarByte repo,
[DevStarByte/arch-repo](https://github.com/DevStarByte/arch-repo). Replace the
old `[byteforge]` section in `/etc/pacman.conf` with:

```ini
[devstarbyte]
SigLevel = Required
Server = https://raw.githubusercontent.com/DevStarByte/arch-repo/main/$arch
```

and trust its signing key once:

```sh
curl -sL https://raw.githubusercontent.com/DevStarByte/arch-repo/main/devstarbyte.asc | sudo pacman-key --add -
sudo pacman-key --lsign-key 2F59DCCF36A08B6C4BB5556DF8E225736ED216F0
```

Then `sudo pacman -Sy byteforge`. The old `[byteforge]` repo in `x86_64/` here
no longer gets updates.

## Modules (IDE packages)

`modules/` is the ByteForge **module registry**: the IDE's Modules view lists these packages under
AVAILABLE and installs, updates and uninstalls them in `~/.config/byteforge/modules/`.

| Module | Version | What it does |
|--------|---------|--------------|
| [CSharpier](modules/csharpier) | 1.0.1 | C# / MSBuild formatter. **Preinstalled** with ByteForge (uninstall and reinstall it like any other). |
| [ReSharper Formatter](modules/resharper-format) | 1.0.0 | C# formatting with JetBrains' `jb cleanupcode` (the Rider/ReSharper formatter); follows .editorconfig and .DotSettings. Needs `dotnet`. |
| [Docker](modules/docker) | 1.2.0 | Docker view for containers, images, volumes, networks and the workspace's Dockerfiles and Compose files; build, Compose, run-with-shell and .NET Docker support commands; Dockerfile highlighting, also recognized in untitled editors. Needs `docker`. |
| [SQL](modules/sql) | 0.2.0 | SQL highlighting (also recognized in untitled editors) and formatting; a Database view with connections from settings, appsettings.json and workspace SQLite files, their tables and columns. Run .sql files (Ctrl+Shift+Enter), Select Top rows, open a shell. SQLite works out of the box; PostgreSQL, MySQL / MariaDB and SQL Server need `psql`, `mysql`, `sqlcmd`. |
| [Aspire](modules/aspire) | 0.1.0 | Aspire view with AppHosts and their running resources (state, health, endpoints, start / stop / restart, logs), the dashboard, and run, new, add, publish and deploy commands. Needs the Aspire CLI. |
| [Azure](modules/azure) | 0.2.0 | Bicep highlighting for `.bicep` and `.bicepparam` files (also recognized in untitled editors) and a Bicep file icon. Nothing to install alongside it. |
| [Lua](modules/lua) | 0.2.0 | Lua highlighting (also recognized in untitled editors), a Lua file icon, "Lua: Run File", "Lua: Check Syntax" and Format Document with StyLua. Needs `python3` and `lua` or `luajit`. |
| [TODO Finder](modules/todos) | 1.0.1 | TODO / FIXME / HACK comments: count in the status bar, "TODOs: List All" with clickable locations. |
| [JSON Formatter](modules/json-format) | 1.0.1 | Format Document for `.json` (appsettings, launchSettings, …); leaves JSON with comments alone. |
| [GUID Generator](modules/guid) | 1.0.1 | "GUID: Insert New GUID" (Ctrl+Alt+G), uppercase, `{braced}` and `new Guid("…")` variants. |
| [.NET Project Files](modules/dotnet-files) | 1.0.1 | Adds .gitignore, .editorconfig, Directory.Build.props, global.json, .gitattributes. |
| [Focus Timer](modules/focus) | 1.0.1 | Pomodoro countdown in the status bar with desktop notifications. |
| [Discord Rich Presence](modules/discord) | 1.1.0 | Shows the file, language and project you're working on in your Discord status; names can be hidden. Needs the Discord desktop app. |
| [Caelestia Theme](modules/caelestia-theme) | 1.0.1 | A color theme that follows your Caelestia dots (caelestia-shell) and restyles ByteForge live when the scheme changes. |

To add or change one: edit `modules/<name>/` (bump `version` in its `module.json`), commit and push.
The **Build module index** workflow regenerates `modules/index.json` (or run `python3 tools/mkindex.py`);
installed copies show an Update button.

## Layout

```
modules/<name>/module.json, …   ByteForge module packages (edit these)
modules/index.json              module registry index (generated)
x86_64/                         old [byteforge] pacman repo (frozen at 0.3.1)
```
