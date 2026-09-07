#!/usr/bin/env bash
# package-plugin.sh — Assemble the OBS plugin bundle on Linux / macOS.
#
# Required env:
#   LIBOBS_INCLUDE_DIR   path to <obs-studio>/libobs   (headers)
#   (Linux)              OBS_STUB_LIB   path to a stub libobs.so with soname libobs.so.0
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

if [[ -z "${LIBOBS_INCLUDE_DIR:-}" ]]; then
  echo "LIBOBS_INCLUDE_DIR is required (path to obs-studio/libobs)." >&2
  exit 1
fi

echo "==> Building Rust engine ..."
"$SCRIPT_DIR/build-engine.sh"

echo "==> Building C plugin ..."
mkdir -p "$ROOT/plugin/build"
pushd "$ROOT/plugin" >/dev/null
if [[ "$(uname)" == "Darwin" ]]; then
  cmake -S . -B build -G "Xcode" \
    "-DLIBOBS_INCLUDE_DIR=$LIBOBS_INCLUDE_DIR"
else
  if [[ -z "${OBS_STUB_LIB:-}" ]]; then
    echo "OBS_STUB_LIB is required on Linux (path to a stub libobs.so)." >&2
    exit 1
  fi
  cmake -S . -B build \
    "-DLIBOBS_INCLUDE_DIR=$LIBOBS_INCLUDE_DIR" \
    "-DOBS_STUB_LIB=$OBS_STUB_LIB"
fi
cmake --build build --config Release
popd >/dev/null

# Assemble the bundle.
if [[ "$(uname)" == "Darwin" ]]; then
  PLATFORM="macos"
else
  PLATFORM="linux"
fi
ARCH="$(uname -m)"
OUT="$ROOT/dist/tv-obsbroadcast-scheduler-${PLATFORM}-${ARCH}"
rm -rf "$OUT"
mkdir -p "$OUT/engine" "$OUT/data/locale"

# cmake build outputs go to one of Release/<x> or ./<x> depending on the
# generator. Search a few candidates and copy the first hit.
for cand in \
  "$ROOT/plugin/build/Release/tv-obsbroadcast-scheduler.so" \
  "$ROOT/plugin/build/tv-obsbroadcast-scheduler.so" \
  "$ROOT/plugin/build/Release/tv-obsbroadcast-scheduler.dylib" \
  "$ROOT/plugin/build/tv-obsbroadcast-scheduler.dylib"
do
  if [[ -f "$cand" ]]; then
    cp "$cand" "$OUT/"
    break
  fi
done
cp "$ROOT/engine/target/release/tv-obsbroadcast-scheduler" "$OUT/engine/"
cp -R "$ROOT/plugin/data/locale/." "$OUT/data/locale/"

# Tarball.
TARBALL="$ROOT/dist/tv-obsbroadcast-scheduler-${PLATFORM}-${ARCH}.tar.gz"
tar -czf "$TARBALL" -C "$ROOT/dist" "$(basename "$OUT")"
echo "==> Plugin bundle: $TARBALL"
