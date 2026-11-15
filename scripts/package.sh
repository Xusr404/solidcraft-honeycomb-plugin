#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="$ROOT/target/wasm32-unknown-unknown/release/solidcraft_honeycomb.wasm"
WASM_TOOLS="${WASM_TOOLS:-wasm-tools}"
DIST="$ROOT/dist"
PLUGIN_ID="solidcraft-honeycomb"

command -v "$WASM_TOOLS" >/dev/null || { echo "wasm-tools 1.263.0 is required." >&2; exit 1; }
test -f "$TARGET" || { echo "Missing $TARGET; build the wasm32-unknown-unknown release target first." >&2; exit 1; }

mkdir -p "$DIST"
WORK="$(mktemp -d "$DIST/.package.XXXXXX")"
trap 'rm -rf -- "$WORK"' EXIT
STAGE="$WORK/$PLUGIN_ID"
ARCHIVE="$WORK/$PLUGIN_ID.zip"

mkdir -p "$STAGE/docs" "$STAGE/examples"
"$WASM_TOOLS" component new "$TARGET" -o "$STAGE/plugin.wasm"
"$WASM_TOOLS" validate "$STAGE/plugin.wasm"
"$WASM_TOOLS" component targets "$ROOT/wit" --world command-plugin "$STAGE/plugin.wasm"
cp "$ROOT/plugin.json" "$ROOT/README.md" "$ROOT/LICENSE" "$ROOT/CHANGELOG.md" "$STAGE/"
cp "$ROOT/docs/LIMITATIONS.md" "$ROOT/docs/honeycomb-screenshot.png" "$STAGE/docs/"
cp "$ROOT/examples/basic-grid.json" "$STAGE/examples/"

if command -v zip >/dev/null 2>&1; then
  (cd "$WORK" && zip -q -r "$ARCHIVE" "$PLUGIN_ID")
elif command -v powershell.exe >/dev/null 2>&1 && command -v cygpath >/dev/null 2>&1; then
  HONEYCOMB_STAGE="$(cygpath -w "$STAGE")" HONEYCOMB_ARCHIVE="$(cygpath -w "$ARCHIVE")" \
    powershell.exe -NoProfile -NonInteractive -Command \
    '$ErrorActionPreference = "Stop"; Compress-Archive -LiteralPath $env:HONEYCOMB_STAGE -DestinationPath $env:HONEYCOMB_ARCHIVE'
else
  echo "zip or Git Bash with Windows PowerShell is required to create the plugin archive." >&2
  exit 1
fi

mv -f -- "$ARCHIVE" "$DIST/$PLUGIN_ID.zip"
echo "Created $DIST/$PLUGIN_ID.zip"
