# Changelog

What changed in each version of ByteForge. Versions before 1.0 are betas, published as pre-releases
on the [Releases](https://github.com/DevStarByte/ByteForge/releases) page.

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
