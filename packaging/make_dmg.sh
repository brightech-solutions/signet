#!/usr/bin/env bash
#
# packaging/make_dmg.sh — builds "Signet.app" (via bundle.sh) and wraps
# it in a drag-to-install "Signet.dmg".
#
# The .dmg mounts with the app on the left and an /Applications shortcut on
# the right, so opening it and dragging the app over is the whole install.
#
# Usage: ./packaging/make_dmg.sh
# Prints the absolute path to the finished .dmg on success.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

command -v hdiutil >/dev/null 2>&1 || {
    echo "error: hdiutil not found (this script requires macOS)" >&2
    exit 1
}

APP_NAME="Signet.app"
DIST_DIR="$REPO_ROOT/dist"
APP_DIR="$DIST_DIR/$APP_NAME"
DMG_NAME="Signet.dmg"
DMG_PATH="$DIST_DIR/$DMG_NAME"
STAGING_DIR="$DIST_DIR/.dmg-staging"

echo "==> Building $APP_NAME"
"$SCRIPT_DIR/bundle.sh"

[ -d "$APP_DIR" ] || {
    echo "error: expected app bundle missing after build: $APP_DIR" >&2
    exit 1
}

echo "==> Staging DMG contents"
rm -rf "$STAGING_DIR" "$DMG_PATH"
mkdir -p "$STAGING_DIR"
cp -R "$APP_DIR" "$STAGING_DIR/"
ln -s /Applications "$STAGING_DIR/Applications"

echo "==> Creating $DMG_NAME"
hdiutil create -volname "Signet" \
    -srcfolder "$STAGING_DIR" \
    -ov -format UDZO \
    "$DMG_PATH"

rm -rf "$STAGING_DIR"

echo "==> Done"
echo "$DMG_PATH"
