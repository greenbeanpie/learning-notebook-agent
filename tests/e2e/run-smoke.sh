#!/usr/bin/env bash
# macOS / Linux counterpart of run-smoke.ps1: mock LLM + lna-devtools serve + Node WebSocket client.
# Usage (repo root):  bash tests/e2e/run-smoke.sh [--pdf]
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
BIN="$ROOT/target/debug/lna-devtools"
[[ -x "$BIN" ]] || { echo "Build first: cargo build -p lna-devtools" >&2; exit 1; }
PDF=0; [[ "${1:-}" == "--pdf" ]] && PDF=1

WORK="$(mktemp -d "${TMPDIR:-/tmp}/lna-e2e-XXXXXX")"
MOCK_PORT=18555
SERVE_ADDR=127.0.0.1:18493
PIDS=()
cleanup() {
  for p in ${PIDS[@]+"${PIDS[@]}"}; do kill "$p" 2>/dev/null || true; done
  echo "Logs: $WORK"
}
trap cleanup EXIT

python3 -I "$HERE/mock_llm.py" "$MOCK_PORT" "$WORK/llm.log" >/dev/null 2>&1 &
PIDS+=($!)
"$BIN" --data-dir "$WORK/data" serve --addr "$SERVE_ADDR" >"$WORK/serve.log" 2>"$WORK/serve.err" &
PIDS+=($!)

URL=""
for _ in $(seq 1 60); do
  sleep 0.5
  URL="$(grep -Eo 'ws://[^ ]+/ws\?token=[0-9a-f-]+' "$WORK/serve.log" 2>/dev/null | head -n1 || true)"
  [[ -n "$URL" ]] && break
done
[[ -n "$URL" ]] || { echo "Dev server did not start; see $WORK/serve.err" >&2; exit 1; }

FAILED=0
node "$HERE/smoke.mjs" "$URL" "$MOCK_PORT" || FAILED=1
mkdir -p "$WORK/scratch"
node "$HERE/extended.mjs" "$URL" "$MOCK_PORT" "$WORK/scratch" || FAILED=1
if [[ "$PDF" == 1 ]]; then
  if [[ ! -f "$ROOT/target/debug/libpdfium.dylib" && ! -f "$ROOT/target/debug/libpdfium.so" ]]; then
    bash "$ROOT/scripts/fetch-pdfium.sh" "$ROOT/target/debug"
  fi
  python3 -I "$HERE/make_pdf.py" "$WORK/t.pdf"
  node "$HERE/pdf.mjs" "$URL" "$WORK/t.pdf" || FAILED=1
fi
exit $FAILED
