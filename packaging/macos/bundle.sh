#!/usr/bin/env bash
# Builds ByteForge.app and a .dmg on macOS.
#
#   packaging/macos/bundle.sh                 # this Mac's architecture
#   UNIVERSAL=1 packaging/macos/bundle.sh     # Apple Silicon + Intel in one binary
#   VERSION=0.4.0 packaging/macos/bundle.sh   # version shown in Finder (default: Cargo.toml)
#
# Output: dist/ByteForge.app and dist/ByteForge-<version>-macos-<arch>.dmg
# The app is ad-hoc signed only; on first launch right-click it and pick Open (or run
# `xattr -dr com.apple.quarantine /Applications/ByteForge.app`).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
VERSION="${VERSION:-$(sed -n '/^\[workspace.package\]/,/^\[/ s/^version *= *"\(.*\)"/\1/p' Cargo.toml)}"

if [ "${UNIVERSAL:-0}" = 1 ]; then
    rustup target add aarch64-apple-darwin x86_64-apple-darwin
    cargo build --release --locked -p byteforge --target aarch64-apple-darwin
    cargo build --release --locked -p byteforge --target x86_64-apple-darwin
    mkdir -p target/universal
    lipo -create -output target/universal/byteforge \
        target/aarch64-apple-darwin/release/byteforge target/x86_64-apple-darwin/release/byteforge
    BIN=target/universal/byteforge
    ARCH=universal
else
    cargo build --release --locked -p byteforge
    BIN=target/release/byteforge
    ARCH="$(uname -m)"
fi

APP=dist/ByteForge.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/themes"
install -m755 "$BIN" "$APP/Contents/MacOS/byteforge"
sed "s/@VERSION@/${VERSION%%-*}/g" packaging/macos/Info.plist > "$APP/Contents/Info.plist"
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
hdiutil create -volname ByteForge -srcfolder "$STAGE" -ov -format UDZO "$DMG"
rm -rf "$STAGE"
echo "Built $APP and $DMG"
