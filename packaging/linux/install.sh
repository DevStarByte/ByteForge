#!/usr/bin/env sh
# Builds ByteForge in release mode and installs it for the current user:
#   ~/.local/lib/byteforge (binary, themes, .NET module host), ~/.local/bin/byteforge, desktop entry, icon.
set -eu

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"

cargo build --release -p byteforge --manifest-path "$ROOT/Cargo.toml"

mkdir -p "$PREFIX/lib/byteforge/themes" "$PREFIX/bin" "$PREFIX/share/applications"
install -m755 "$ROOT/target/release/byteforge" "$PREFIX/lib/byteforge/byteforge"
install -m644 "$ROOT"/themes/*.json "$PREFIX/lib/byteforge/themes/"
rm -rf "$PREFIX/lib/byteforge/modules" "$PREFIX/lib/byteforge/modulehost"
dotnet publish "$ROOT/modulehost/ByteForge.ModuleHost.csproj" -c Release -o "$PREFIX/lib/byteforge/modulehost" -nologo
ln -sf "$PREFIX/lib/byteforge/byteforge" "$PREFIX/bin/byteforge"
sed "s|Exec=byteforge|Exec=$PREFIX/bin/byteforge|" "$ROOT/packaging/linux/byteforge.desktop" > "$PREFIX/share/applications/byteforge.desktop"
install -Dm644 "$ROOT/packaging/linux/byteforge.svg" "$PREFIX/share/icons/hicolor/scalable/apps/byteforge.svg"
for size in 16 24 32 48 64 128 256 512; do
  install -Dm644 "$ROOT/packaging/linux/icons/byteforge-$size.png" "$PREFIX/share/icons/hicolor/${size}x${size}/apps/byteforge.png"
done

echo "Installed. Run 'byteforge [folder]' (make sure $PREFIX/bin is on PATH)."
command -v csharp-ls >/dev/null 2>&1 || echo "Tip: install the C# language server: dotnet tool install -g csharp-ls"
command -v jb >/dev/null 2>&1 || echo "Tip: install the default formatter: dotnet tool install -g JetBrains.ReSharper.GlobalTools"
