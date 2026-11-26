#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_APP="${1:-"$ROOT_DIR/dist/LocalNotebook.app"}"

cd "$ROOT_DIR"

echo "Building lna-ffi (release)..."
cargo build -p lna-ffi --release

echo "Building LocalNotebook SwiftUI app (release)..."
swift build --package-path apps/macos/LocalNotebook -c release

BIN_PATH="$ROOT_DIR/apps/macos/LocalNotebook/.build/release/LocalNotebook"
DYLIB_PATH="$ROOT_DIR/target/release/liblna_ffi.dylib"

if [[ ! -f "$BIN_PATH" ]]; then
  echo "Error: Swift binary not found at $BIN_PATH" >&2
  exit 1
fi

if [[ ! -f "$DYLIB_PATH" ]]; then
  echo "Error: liblna_ffi.dylib not found at $DYLIB_PATH" >&2
  exit 1
fi

echo "Assembling $OUT_APP..."
rm -rf "$OUT_APP"
mkdir -p "$OUT_APP/Contents/MacOS" "$OUT_APP/Contents/Frameworks" "$OUT_APP/Contents/Resources"

cp "$BIN_PATH" "$OUT_APP/Contents/MacOS/LocalNotebook"
chmod +x "$OUT_APP/Contents/MacOS/LocalNotebook"

cp "$DYLIB_PATH" "$OUT_APP/Contents/Frameworks/liblna_ffi.dylib"
chmod +x "$OUT_APP/Contents/Frameworks/liblna_ffi.dylib"

# PDF import needs PDFium; it is looked up next to the executable (Contents/MacOS).
echo "Fetching PDFium for PDF import..."
bash "$ROOT_DIR/scripts/fetch-pdfium.sh" "$OUT_APP/Contents/MacOS"
chmod +x "$OUT_APP/Contents/MacOS/libpdfium.dylib"

LOGO_SVG="$ROOT_DIR/assets/logo.svg"
if [[ -f "$LOGO_SVG" ]]; then
  cp "$LOGO_SVG" "$OUT_APP/Contents/Resources/logo.svg"
  echo "Generating AppIcon.icns from assets/logo.svg..."
  ICON_TMP="$(mktemp -d)"
  ICONSET_DIR="$ICON_TMP/AppIcon.iconset"
  mkdir -p "$ICONSET_DIR"
  if qlmanage -t -s 1024 -o "$ICON_TMP" "$LOGO_SVG" >/dev/null 2>&1 && [[ -f "$ICON_TMP/logo.svg.png" ]]; then
    BASE_PNG="$ICON_TMP/logo.svg.png"
    for sz in 16 32 128 256 512; do
      sips -z "$sz" "$sz" "$BASE_PNG" --out "$ICONSET_DIR/icon_${sz}x${sz}.png" >/dev/null
      sz2=$((sz * 2))
      sips -z "$sz2" "$sz2" "$BASE_PNG" --out "$ICONSET_DIR/icon_${sz}x${sz}@2x.png" >/dev/null
    done
    iconutil -c icns "$ICONSET_DIR" -o "$OUT_APP/Contents/Resources/AppIcon.icns"
  fi
  rm -rf "$ICON_TMP"
fi

cat > "$OUT_APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleExecutable</key>
    <string>LocalNotebook</string>
    <key>CFBundleIdentifier</key>
    <string>com.localnotebook.agent</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>LocalNotebook</string>
    <key>CFBundleDisplayName</key>
    <string>Local Notebook</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>0.1.0</string>
    <key>CFBundleVersion</key>
    <string>1</string>
    <key>LSMinimumSystemVersion</key>
    <string>14.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSPrincipalClass</key>
    <string>NSApplication</string>
</dict>
</plist>
PLIST

echo "Relocating liblna_ffi.dylib into @rpath..."
install_name_tool -id "@rpath/liblna_ffi.dylib" "$OUT_APP/Contents/Frameworks/liblna_ffi.dylib"

OLD_DYLIB_REF="$(otool -L "$OUT_APP/Contents/MacOS/LocalNotebook" | awk '/liblna_ffi\.dylib/ {print $1; exit}')"
if [[ -n "$OLD_DYLIB_REF" && "$OLD_DYLIB_REF" != "@rpath/liblna_ffi.dylib" ]]; then
  install_name_tool -change "$OLD_DYLIB_REF" "@rpath/liblna_ffi.dylib" "$OUT_APP/Contents/MacOS/LocalNotebook"
fi

if ! otool -l "$OUT_APP/Contents/MacOS/LocalNotebook" | grep -q "@executable_path/../Frameworks"; then
  install_name_tool -add_rpath "@executable_path/../Frameworks" "$OUT_APP/Contents/MacOS/LocalNotebook"
fi

echo "Ad-hoc signing app bundle..."
codesign --force --deep --sign - "$OUT_APP"
codesign --verify --deep --strict "$OUT_APP"

echo "Done: $OUT_APP"
