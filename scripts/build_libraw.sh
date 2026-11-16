#!/usr/bin/env bash
# Builds the pinned LibRaw commit into PREFIX for the desktop release builds
# (Linux AppImage, Windows, macOS). Released LibRaw packages lack cameras that
# master supports, such as the Sony A7 V and its ARW6 compression, so every
# platform builds this commit from source. Extra arguments go to ./configure.
#
# Usage: scripts/build_libraw.sh PREFIX [CONFIGURE_ARG...]
set -euo pipefail

# Keep in sync with workspace.metadata.libraw_revision in Cargo.toml, the
# Android pin in android/app/src/main/cpp/CMakeLists.txt and the Flatpak
# manifest.
LIBRAW_SOURCE_REVISION=a9a14b1970d188c25dade81f8326ff00a6e33649
LIBRAW_ARCHIVE_SHA256=453b1311b4825cc1946f114b5a0bcf0d4ad62a52c667dd5cc8b2b70e28342020

prefix="${1:?usage: scripts/build_libraw.sh PREFIX [CONFIGURE_ARG...]}"
shift

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
workspace_revision="$(sed -n 's/^libraw_revision = "\(.*\)"$/\1/p' "$repository_root/Cargo.toml")"
if [ "$workspace_revision" != "$LIBRAW_SOURCE_REVISION" ]; then
  echo "error: Cargo.toml pins LibRaw '$workspace_revision', but this script pins $LIBRAW_SOURCE_REVISION" >&2
  exit 1
fi

# The pinned master commit reports version 0.23.0, which no release carries,
# so this stamp rather than pkg-config identifies the source a cached prefix
# was built from.
stamp="$prefix/share/calibraw/libraw-revision"
if [ -f "$prefix/lib/pkgconfig/libraw.pc" ] \
  && [ "$(cat "$stamp" 2>/dev/null || true)" = "$LIBRAW_SOURCE_REVISION" ]; then
  echo "Using cached LibRaw $LIBRAW_SOURCE_REVISION from $prefix"
  exit 0
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

python3 "$repository_root/scripts/bootstrap_download.py" \
  "https://github.com/LibRaw/LibRaw/archive/$LIBRAW_SOURCE_REVISION.tar.gz" \
  "$work/libraw.tar.gz" "$LIBRAW_ARCHIVE_SHA256"
mkdir "$work/source"
tar -xzf "$work/libraw.tar.gz" --strip-components=1 -C "$work/source"

jobs="$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 2)"
(
  cd "$work/source"
  autoreconf --install --force
  ./configure --prefix="$prefix" --disable-static --disable-examples "$@"
  make --jobs="$jobs"
  make install
)

mkdir -p "$(dirname "$stamp")"
printf '%s\n' "$LIBRAW_SOURCE_REVISION" > "$stamp"
echo "Installed LibRaw $LIBRAW_SOURCE_REVISION into $prefix"
