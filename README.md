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
| [Lua](modules/lua) | 0.2.1 | Lua highlighting (also recognized in untitled editors), a Lua file icon, "Lua: Run File", "Lua: Check Syntax" and Format Document with StyLua. Needs `python3` and `lua` or `luajit`. |
| [HTTP Client](modules/http-client) | 0.1.0 | Sends the requests in `.http` / `.rest` files (the Visual Studio / Rider format): an HTTP view with every request in the workspace, "HTTP: Send Request…" (Ctrl+Alt+Enter), Send All; file variables, environments from `http-client.env.json`, values from earlier responses, `{{$guid}}` and friends. Responses open in an editor (status, headers, timing, pretty JSON). Needs `python3`. |
| [Protocol Buffers](modules/protobuf) | 0.1.0 | `.proto` highlighting (gRPC services, messages, enums, options) and file icon; Format Document with `buf` or `clang-format`. |
| [GitHub](modules/github-actions) | 0.2.0 | A GitHub view for the folder's repository: pipelines (runs of the branch and per workflow, jobs, steps, logs at the first error; re-run, cancel, Run Workflow…), open pull requests with their checks (Check Out, the branch's PR in the status bar), open issues (Create Branch), the latest releases. Public repositories work without a token; logs and the rest need `github.token`, `GH_TOKEN` or `gh`. Needs `git`. |
| [Git Hooks](modules/git-hooks) | 0.1.0 | A Git Hooks view with the repository's hooks (`core.hooksPath` / `.husky` too): run one as git would, turn it off and on, make it executable; "New Hook…" from templates (dotnet format, build, test, Conventional Commits, issue number from the branch); Husky.NET tasks. Needs `git`. |
| [CSV](modules/csv) | 0.1.0 | Rainbow columns for `.csv` / `.tsv` (quoted fields kept together; semicolon files from a German Excel split at `;`), a CSV view with each column's type, range, empty and distinct values, "Show as Table" and "Convert to JSON". |
| [Log Files](modules/log-files) | 0.1.0 | `.log` highlighting (levels, timestamps, exceptions, stack frames; ASP.NET Core, Serilog, NLog, GitHub Actions), "Log: List Errors" with clickable locations, "Log: Filter…", "Log: Follow" (tail -f), error and warning counts in the status bar. |
| [TODO Finder](modules/todos) | 1.0.1 | TODO / FIXME / HACK comments: count in the status bar, "TODOs: List All" with clickable locations. |
| [JSON Formatter](modules/json-format) | 1.0.1 | Format Document for `.json` (appsettings, launchSettings, …); leaves JSON with comments alone. |
| [GUID Generator](modules/guid) | 1.0.1 | "GUID: Insert New GUID" (Ctrl+Alt+G), uppercase, `{braced}` and `new Guid("…")` variants. |
| [.NET Project Files](modules/dotnet-files) | 1.0.1 | Adds .gitignore, .editorconfig, Directory.Build.props, global.json, .gitattributes. |
| [Focus Timer](modules/focus) | 1.0.1 | Pomodoro countdown in the status bar with desktop notifications. |
| [Discord Rich Presence](modules/discord) | 1.1.0 | Shows the file, language and project you're working on in your Discord status; names can be hidden. Needs the Discord desktop app. |
| [macOS Theme](modules/macos-theme) | 0.1.0 | A theme that looks like macOS and follows System Settings live: Light / Dark (Auto too), the accent color, Reduce Transparency, Increase Contrast. Translucent sidebar, Xcode's syntax colors, SF Pro and SF Mono. |
| [Windows Theme](modules/windows-theme) | 0.1.0 | A theme that looks like Windows 11 and follows Windows' settings live: app mode, accent color and its shades, transparency (Mica), accent on title bars, high contrast. Visual Studio 2022's syntax colors, Segoe UI Variable and Cascadia Mono. |
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
