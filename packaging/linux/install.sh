#!/usr/bin/env sh
# Builds ByteForge in release mode and installs it for the current user:
#   ~/.local/lib/byteforge (binary + themes), ~/.local/bin/byteforge, desktop entry.
set -eu

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"

cargo build --release -p byteforge --manifest-path "$ROOT/Cargo.toml"

mkdir -p "$PREFIX/lib/byteforge/themes" "$PREFIX/bin" "$PREFIX/share/applications"
install -m755 "$ROOT/target/release/byteforge" "$PREFIX/lib/byteforge/byteforge"
install -m644 "$ROOT"/themes/*.json "$PREFIX/lib/byteforge/themes/"
ln -sf "$PREFIX/lib/byteforge/byteforge" "$PREFIX/bin/byteforge"
sed "s|Exec=byteforge|Exec=$PREFIX/bin/byteforge|" "$ROOT/packaging/linux/byteforge.desktop" > "$PREFIX/share/applications/byteforge.desktop"

echo "Installed. Run 'byteforge [folder]' (make sure $PREFIX/bin is on PATH)."
command -v csharp-ls >/dev/null 2>&1 || echo "Tip: install the C# language server: dotnet tool install -g csharp-ls"
