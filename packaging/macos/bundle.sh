#!/usr/bin/env bash
# Builds ByteForge.app and a .dmg on macOS.
#
#   packaging/macos/bundle.sh                 # this Mac's architecture
#   ARCH=arm64 packaging/macos/bundle.sh      # Apple Silicon
#   ARCH=x64 packaging/macos/bundle.sh        # Intel (cross-compiles fine on Apple Silicon)
#   ARCH=universal packaging/macos/bundle.sh  # Apple Silicon + Intel in one binary
#   VERSION=0.4.0 packaging/macos/bundle.sh   # version shown in Finder (default: Cargo.toml)
#
# Output: dist/ByteForge.app and dist/ByteForge-<version>-macos-<arm64|x64|universal>.dmg
# The app is ad-hoc signed only; on first launch right-click it and pick Open (or run
# `xattr -dr com.apple.quarantine /Applications/ByteForge.app`).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
VERSION="${VERSION:-$(sed -n '/^\[workspace.package\]/,/^\[/ s/^version *= *"\(.*\)"/\1/p' Cargo.toml)}"

if [ -z "${ARCH:-}" ]; then
    case "$(uname -m)" in
        arm64|aarch64) ARCH=arm64 ;;
        x86_64) ARCH=x64 ;;
        *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
    esac
fi

build() { # build <rust target>
    rustup target add "$1"
    cargo build --release --locked -p byteforge --target "$1"
}

case "$ARCH" in
    arm64)
        build aarch64-apple-darwin
        BIN=target/aarch64-apple-darwin/release/byteforge ;;
    x64)
        build x86_64-apple-darwin
        BIN=target/x86_64-apple-darwin/release/byteforge ;;
    universal)
        build aarch64-apple-darwin
        build x86_64-apple-darwin
        mkdir -p target/universal
        lipo -create -output target/universal/byteforge \
            target/aarch64-apple-darwin/release/byteforge target/x86_64-apple-darwin/release/byteforge
        BIN=target/universal/byteforge ;;
    *) echo "ARCH must be arm64, x64 or universal (got: $ARCH)" >&2; exit 1 ;;
esac

APP=dist/ByteForge.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/themes"
install -m755 "$BIN" "$APP/Contents/MacOS/byteforge"
sed "s/@VERSION@/${VERSION%%-*}/g" packaging/macos/Info.plist > "$APP/Contents/Info.plist"
cp packaging/macos/ByteForge.icns "$APP/Contents/Resources/"
cp themes/*.json "$APP/Contents/Resources/themes/"
cp -R modules "$APP/Contents/Resources/modules"
cp README.md LICENSE "$APP/Contents/Resources/"
# Apple Silicon refuses to run unsigned code; an ad-hoc signature is enough for local use.
codesign --force --deep --sign - "$APP"

DMG="dist/ByteForge-$VERSION-macos-$ARCH.dmg"
STAGE="$(mktemp -d)"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG"
# hdiutil's own size estimate for -srcfolder is often too small on CI runners
# ("No space left on device"), so size the image from the staged folder plus headroom.
SIZE_MB=$(( $(du -sm "$STAGE" | cut -f1) * 12 / 10 + 50 ))
hdiutil create -volname ByteForge -srcfolder "$STAGE" -fs HFS+ -size "${SIZE_MB}m" -ov -format UDZO "$DMG"
rm -rf "$STAGE"
echo "Built $APP and $DMG"
