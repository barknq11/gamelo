#!/usr/bin/env bash
# Build a portable, single-file AppImage for ProtonLaunch.
#
# The result runs on any x86_64 distro with glibc >= 2.17 (roughly 2013+),
# because the binary is linked against an old glibc baseline using zig.
#
# Requirements (all installable without root):
#   - rustup (rust toolchain)
#   - zig 0.17.x on PATH           (https://ziglang.org/download/)
#   - cargo-zigbuild               (cargo install cargo-zigbuild)
#   - appimagetool                 (https://github.com/AppImage/appimagetool)
#
# Usage:  packaging/build-appimage.sh [path-to-appimagetool]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PKG="$ROOT/packaging"
TARGET="x86_64-unknown-linux-gnu.2.17"
TRIPLE="x86_64-unknown-linux-gnu"
OUT="$ROOT/dist"
APPDIR="$ROOT/target/appimage/ProtonLaunch.AppDir"

APPIMAGETOOL="${1:-$(command -v appimagetool || true)}"
if [ -z "$APPIMAGETOOL" ]; then
    for c in "$HOME/.local/appimage/appimagetool" "$PKG/appimagetool"; do
        [ -x "$c" ] && APPIMAGETOOL="$c" && break
    done
fi
if [ -z "$APPIMAGETOOL" ]; then
    echo "error: appimagetool not found; pass its path as the first argument" >&2
    exit 1
fi

echo "==> Building portable binary ($TARGET)"
(cd "$ROOT" && cargo zigbuild --release --target "$TARGET")

BIN="$ROOT/target/$TRIPLE/release/protonlaunch"
[ -x "$BIN" ] || { echo "error: $BIN not found" >&2; exit 1; }

echo "==> Assembling AppDir"
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin"
cp "$BIN" "$APPDIR/usr/bin/protonlaunch"
cp "$PKG/AppRun" "$APPDIR/AppRun"
chmod +x "$APPDIR/AppRun"
cp "$PKG/protonlaunch.desktop" "$APPDIR/protonlaunch.desktop"
cp "$PKG/protonlaunch.png" "$APPDIR/protonlaunch.png"

mkdir -p "$OUT"
echo "==> Packaging AppImage"
ARCH=x86_64 "$APPIMAGETOOL" --no-appstream "$APPDIR" "$OUT/ProtonLaunch-x86_64.AppImage"

echo "==> Done: $OUT/ProtonLaunch-x86_64.AppImage"
ls -lh "$OUT/ProtonLaunch-x86_64.AppImage"
