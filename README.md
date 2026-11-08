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
| [byteforge](pkgs/byteforge) | 0.1.0 | A modular, themeable IDE for C# built with Rust and GPUI. |

## Layout

```
pkgs/<name>/PKGBUILD                      package sources (edit these)
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
