#!/usr/bin/env bash
# Downloads the pinned PDFium build for macOS and copies libpdfium.dylib into the given directories.
# Same pin as scripts/fetch-pdfium.ps1 (bblanchon/pdfium-binaries); SHA-256 is verified before extraction.
#
# Usage: scripts/fetch-pdfium.sh [--arch arm64|x64] DEST [DEST...]
set -euo pipefail

TAG='chromium/8165'
ARCH="$(uname -m)"; [[ "$ARCH" == "x86_64" ]] && ARCH=x64
if [[ "${1:-}" == "--arch" ]]; then ARCH="$2"; shift 2; fi
[[ $# -ge 1 ]] || { echo "usage: $0 [--arch arm64|x64] DEST [DEST...]" >&2; exit 2; }

case "$ARCH" in
  arm64) SHA='112e74d717f3e0310273b70f3c8780e8a61ff11b34834124465c8305c841d350' ;;
  x64)   SHA='07285915b564e46a432a9909dbc1f15892885ca6489e3faeefc70dee77ed371a' ;;
  *) echo "unsupported arch: $ARCH" >&2; exit 2 ;;
esac
ASSET="pdfium-mac-$ARCH.tgz"
URL="https://github.com/bblanchon/pdfium-binaries/releases/download/$TAG/$ASSET"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
echo "Downloading $URL"
curl -fsSL -o "$WORK/$ASSET" "$URL"
ACTUAL="$(shasum -a 256 "$WORK/$ASSET" | awk '{print $1}')"
if [[ "$ACTUAL" != "$SHA" ]]; then
  echo "SHA-256 mismatch for $ASSET: expected $SHA, actual $ACTUAL" >&2
  exit 1
fi
mkdir "$WORK/x"
tar -xzf "$WORK/$ASSET" -C "$WORK/x"
for d in "$@"; do
  mkdir -p "$d"
  cp -f "$WORK/x/lib/libpdfium.dylib" "$d/"
  cp -f "$WORK/x/LICENSE" "$d/PDFium-LICENSE.txt"
  echo "libpdfium.dylib ($TAG, $ARCH) -> $d"
done
