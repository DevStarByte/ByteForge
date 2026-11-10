<div align="center">

# ByteForge

**A fast, modular, VS Code–style IDE for C#, built from scratch in Rust.**

[![Version](https://img.shields.io/github/v/tag/DevStarByte/ByteForge?label=version&color=blue&sort=semver)](https://github.com/DevStarByte/ByteForge/tags)
[![Rust](.github/assets/rust-badge.svg)](https://www.rust-lang.org/)
[![GPUI](https://img.shields.io/badge/UI-GPUI-084CCF?logo=zedindustries&logoColor=white)](https://github.com/zed-industries/zed)
[![.NET](https://img.shields.io/badge/.NET-SDK-512BD4?logo=dotnet)](https://dotnet.microsoft.com/)
[![Linux](https://img.shields.io/badge/Linux-X11%20%7C%20Wayland-FCC624?logo=linux&logoColor=black)](#install)
[![Windows](https://img.shields.io/badge/Windows-supported-0078D6?logo=windows&logoColor=white)](#install)
[![License: GPL-2.0](https://img.shields.io/badge/license-GPL--2.0-blue)](LICENSE)

</div>

ByteForge is a GPU-rendered IDE for C# and .NET, built on [GPUI](https://github.com/zed-industries/zed), the UI
framework behind Zed. Every part of the window can be restyled and rearranged through theme files that reload
live, and modules written in any language can extend it.

## Features

- **Editor:** multiple cursors, folding, minimap, snippets, find & replace, and tree-sitter highlighting that
  runs in the background
- **C#:** completion, hover, go to definition, find references, rename, quick fixes and inlay hints, powered by
  `csharp-ls` (Roslyn)
- **Razor and MSBuild:** highlighting for `.razor`, `.cshtml`, `.csproj` and other XML project files
- **Debugger:** breakpoints, stepping, variables and a Debug Console through netcoredbg; F5 runs ASP.NET Core apps
  with their launch profile
- **Tests:** a Test Explorer for xUnit, NUnit and MSTest, with run and debug for any test
- **Git:** a Source Control view, change markers in the gutter, diffs, branches, and pull / push / fetch
- **Docker:** containers, images, volumes and Compose, plus Dockerfile generation for .NET projects
- **Terminal:** your own shell on a real PTY, with true color and several tabs
- **Formatting:** CSharpier by default, or Roslyn

## Install

### Arch Linux

Add the DevStarByte repo to `/etc/pacman.conf` (one repo for all DevStarByte projects):

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

Pre-releases (betas) are marked as such on the Releases page; each release also carries the Arch package
(`sudo pacman -U byteforge-*.pkg.tar.zst`) for trying a beta without the pacman repo.

### From source

You need Rust 1.85+, the .NET SDK and `csharp-ls`:

```sh
dotnet tool install -g csharp-ls
cargo run --release -p byteforge -- ~/code/my-project
```

`./packaging/linux/install.sh` installs a build to `~/.local`, and `packaging/arch` has a PKGBUILD
(`makepkg -si`).

**Optional:**
- For debugging, install [netcoredbg](https://github.com/Samsung/netcoredbg) (AUR: `netcoredbg`).
- For ASP.NET Core and Blazor projects, install the ASP.NET Core runtime and targeting pack.

> [!NOTE]
> On Linux, GPUI renders through Vulkan, so you need the Vulkan driver for your GPU (`vulkan-radeon`,
> `vulkan-intel` or `nvidia-utils`).

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

- Git pull is fast-forward only, and there's no merge-conflict editor or blame yet.
- Razor files have no completion or hover yet; errors show up after a build.
- No conditional breakpoints or watch expressions yet. Blazor WebAssembly can't be debugged.
- Very large solutions take a while before diagnostics appear.

## License

ByteForge is licensed under the [GNU General Public License v2.0](LICENSE).
