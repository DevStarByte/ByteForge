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
| [CSharpier](modules/csharpier) | 1.0.0 | C# / MSBuild formatter. **Preinstalled** with ByteForge (uninstall and reinstall it like any other). |
| [ReSharper Formatter](modules/resharper-format) | 1.0.0 | C# formatting with JetBrains' `jb cleanupcode` (the Rider/ReSharper formatter); follows .editorconfig and .DotSettings. Needs `dotnet`. |
| [Docker](modules/docker) | 1.1.2 | Docker view for containers, images, volumes, networks and the workspace's Dockerfiles and Compose files; build, Compose, run-with-shell and .NET Docker support commands. **Preinstalled** with ByteForge. Needs `docker`. |
| [SQL](modules/sql) | 0.1.1 | SQL highlighting and formatting; a Database view with connections from settings, appsettings.json and workspace SQLite files, their tables and columns. Run .sql files (Ctrl+Shift+Enter), Select Top rows, open a shell. **Preinstalled** with ByteForge. SQLite works out of the box; PostgreSQL, MySQL / MariaDB and SQL Server need `psql`, `mysql`, `sqlcmd`. |
| [Aspire](modules/aspire) | 0.1.0 | Aspire view with AppHosts and their running resources (state, health, endpoints, start / stop / restart, logs), the dashboard, and run, new, add, publish and deploy commands. **Preinstalled** with ByteForge. Needs the Aspire CLI. |
| [Azure](modules/azure) | 0.1.0 | Bicep highlighting for `.bicep` and `.bicepparam` files and a Bicep file icon. **Preinstalled** with ByteForge. Nothing to install alongside it. |
| [TODO Finder](modules/todos) | 1.0.0 | TODO / FIXME / HACK comments: count in the status bar, "TODOs: List All" with clickable locations. |
| [JSON Formatter](modules/json-format) | 1.0.0 | Format Document for `.json` (appsettings, launchSettings, …); leaves JSON with comments alone. |
| [GUID Generator](modules/guid) | 1.0.0 | "GUID: Insert New GUID" (Ctrl+Alt+G), uppercase, `{braced}` and `new Guid("…")` variants. |
| [.NET Project Files](modules/dotnet-files) | 1.0.0 | Adds .gitignore, .editorconfig, Directory.Build.props, global.json, .gitattributes. |
| [Focus Timer](modules/focus) | 1.0.0 | Pomodoro countdown in the status bar with desktop notifications. |

To add or change one: edit `modules/<name>/` (bump `version` in its `module.json`), commit and push.
The **Build module index** workflow regenerates `modules/index.json` (or run `python3 tools/mkindex.py`);
installed copies show an Update button.

## Layout

```
modules/<name>/module.json, …   ByteForge module packages (edit these)
modules/index.json              module registry index (generated)
x86_64/                         old [byteforge] pacman repo (frozen at 0.3.1)
```
