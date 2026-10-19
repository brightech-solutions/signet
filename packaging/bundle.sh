#!/usr/bin/env bash
#
# packaging/bundle.sh — builds a universal (arm64 + x86_64), ad-hoc-signed
# "PDF Utility.app" bundle from source.
#
# Steps:
#   1. Build release binaries for aarch64-apple-darwin and x86_64-apple-darwin
#      (installing the rustup targets first if missing).
#   2. Combine them into one universal binary with `lipo -create`.
#   3. Assemble the PDF Utility.app/Contents/{MacOS,Resources,Frameworks}
#      bundle structure.
#   4. Copy the universal binary into Contents/MacOS/.
#   5. Copy packaging/Info.plist into Contents/.
#   6. Combine both vendored pdfium dylib arch slices into a universal
#      Contents/Frameworks/libpdfium.dylib via `lipo`.
#   7. Ad-hoc codesign the framework and the whole app bundle.
#
# The resulting app is fully offline: no installer, no background process,
# and the pdfium library is bundled inside Contents/Frameworks rather than
# installed system-wide.
#
# Usage: ./packaging/bundle.sh
# Prints the absolute path to the finished .app bundle on success.

set -euo pipefail

# --- Resolve paths -----------------------------------------------------

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

# `cargo`/`rustup` live in ~/.cargo/bin, which isn't always on PATH for
# non-interactive shells.
if ! command -v cargo >/dev/null 2>&1 && [ -x "$HOME/.cargo/bin/cargo" ]; then
    export PATH="$HOME/.cargo/bin:$PATH"
fi

command -v cargo >/dev/null 2>&1 || {
    echo "error: cargo not found on PATH" >&2
    exit 1
}
command -v rustup >/dev/null 2>&1 || {
    echo "error: rustup not found on PATH" >&2
    exit 1
}
command -v lipo >/dev/null 2>&1 || {
    echo "error: lipo not found (requires Xcode command line tools)" >&2
    exit 1
}
command -v codesign >/dev/null 2>&1 || {
    echo "error: codesign not found (requires Xcode command line tools)" >&2
    exit 1
}

BIN_NAME="pdf_utility"
APP_NAME="PDF Utility.app"
DIST_DIR="$REPO_ROOT/dist"
APP_DIR="$DIST_DIR/$APP_NAME"
TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)

# --- Step 1: build release binaries for both targets --------------------

for target in "${TARGETS[@]}"; do
    if ! rustup target list --installed | grep -qx "$target"; then
        echo "==> Installing missing rustup target: $target"
        rustup target add "$target"
    fi

    echo "==> Building release binary for $target"
    cargo build --release --target "$target"
done

ARM64_BIN="$REPO_ROOT/target/aarch64-apple-darwin/release/$BIN_NAME"
X86_64_BIN="$REPO_ROOT/target/x86_64-apple-darwin/release/$BIN_NAME"

for f in "$ARM64_BIN" "$X86_64_BIN"; do
    [ -f "$f" ] || {
        echo "error: expected build output missing: $f" >&2
        exit 1
    }
done

# --- Step 2+3: assemble the .app bundle skeleton -------------------------

echo "==> Assembling $APP_NAME bundle structure"
rm -rf "$APP_DIR"
mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources" "$APP_DIR/Contents/Frameworks"

# --- Step 4: universal binary via lipo -----------------------------------

echo "==> Creating universal binary"
lipo -create -output "$APP_DIR/Contents/MacOS/$BIN_NAME" "$ARM64_BIN" "$X86_64_BIN"
chmod +x "$APP_DIR/Contents/MacOS/$BIN_NAME"

# --- Step 5: Info.plist + app icon -----------------------------------------

cp "$SCRIPT_DIR/Info.plist" "$APP_DIR/Contents/Info.plist"

ICON_SRC="$REPO_ROOT/assets/icon/AppIcon.icns"
[ -f "$ICON_SRC" ] || {
    echo "error: expected app icon missing: $ICON_SRC" >&2
    exit 1
}
cp "$ICON_SRC" "$APP_DIR/Contents/Resources/AppIcon.icns"

# --- Step 6: universal pdfium dylib via lipo ------------------------------

ARM64_DYLIB="$REPO_ROOT/vendor/pdfium/macos-arm64/libpdfium.dylib"
X86_64_DYLIB="$REPO_ROOT/vendor/pdfium/macos-x86_64/libpdfium.dylib"

for f in "$ARM64_DYLIB" "$X86_64_DYLIB"; do
    [ -f "$f" ] || {
        echo "error: expected vendored pdfium dylib missing: $f" >&2
        exit 1
    }
done

echo "==> Creating universal pdfium dylib"
FRAMEWORK_DYLIB="$APP_DIR/Contents/Frameworks/libpdfium.dylib"
lipo -create -output "$FRAMEWORK_DYLIB" "$ARM64_DYLIB" "$X86_64_DYLIB"

# Give the bundled dylib a clean, relocatable ID. This is hygiene rather
# than a functional requirement: pdf_utility resolves the dylib's absolute
# path itself at runtime (see src/document.rs: vendored_library_path) and
# dlopen()s it directly via pdfium-render's dynamic binding, so there is no
# load command in the pdf_utility binary pointing at this dylib for
# install_name_tool to fix up.
install_name_tool -id "@rpath/libpdfium.dylib" "$FRAMEWORK_DYLIB"

# --- Step 7: ad-hoc codesign ----------------------------------------------

echo "==> Ad-hoc codesigning framework and app bundle"
codesign --force --sign - "$FRAMEWORK_DYLIB"
codesign --force --deep --sign - "$APP_DIR"

echo "==> Done"
echo "$APP_DIR"
