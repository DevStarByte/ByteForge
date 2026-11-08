# ByteForge Package Repo

This is the `packages` branch of ByteForge: the pacman repository only, kept
apart from the IDE on `master` so cloning the source doesn't drag every built
package along. pacman downloads straight from here.

## Install on Arch Linux

Add the repo to `/etc/pacman.conf`:

```ini
[byteforge]
SigLevel = Optional TrustAll
Server = https://raw.githubusercontent.com/DevStarByte/ByteForge/packages/$arch
```

Then:

```sh
sudo pacman -Sy byteforge
dotnet tool install -g csharp-ls      # C# language server
```

Updates arrive with `sudo pacman -Syu`. Remove with `sudo pacman -R byteforge`.

## Packages

| Package | Version | Description |
|---------|---------|-------------|
| [byteforge](pkgs/byteforge) | 0.2.0 | A modular, themeable IDE for C# built with Rust and GPUI. |

## Modules (IDE packages)

`modules/` is the ByteForge **module registry**: the IDE's Modules view lists these packages under
AVAILABLE and installs, updates and uninstalls them in `~/.config/byteforge/modules/`.

| Module | Version | What it does |
|--------|---------|--------------|
| [CSharpier](modules/csharpier) | 1.0.0 | C# / MSBuild formatter. **Preinstalled** with ByteForge (uninstall and reinstall it like any other). |
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
pkgs/<name>/PKGBUILD                      pacman package sources (edit these)
modules/<name>/module.json, …             ByteForge module packages (edit these)
modules/index.json                        module registry index (generated)
x86_64/<name>-<ver>-<rel>-x86_64.pkg.tar.zst  built packages  (generated)
x86_64/byteforge.db, byteforge.files      repo database   (generated)
```

## Adding or changing a package

1. Edit `pkgs/<name>/PKGBUILD`. When only the packaging changes, bump
   `pkgrel`; for a new release, tag master (`git tag v0.2.0 && git push
   origin v0.2.0`) and set `pkgver` to match.
2. Commit and push. The **Build packages** workflow builds it in an Arch
   container, updates `x86_64/` with `repo-add` and commits it back.
   Computers pick the change up on their next `pacman -Syu`.

To build by hand instead: `cd pkgs/byteforge && makepkg -si`.

For signed packages, add an armored GPG private key without passphrase as the
secret `GPG_PRIVATE_KEY`; then import the public key on each machine
(`sudo pacman-key --add key.asc && sudo pacman-key --lsign-key <id>`) and set
`SigLevel = Required`.
