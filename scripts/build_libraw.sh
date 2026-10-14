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
LIBRAW_SOURCE_REVISION=830ac963b40223c6a25add5be65036fa8d56e2d6
LIBRAW_ARCHIVE_SHA256=dd77deb5e7e290d2216ad8688ec4ac7dccdeea3def0927076a63a61d326e212a

prefix="${1:?usage: scripts/build_libraw.sh PREFIX [CONFIGURE_ARG...]}"
shift

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
workspace_revision="$(sed -n 's/^libraw_revision = "\(.*\)"$/\1/p' "$repository_root/Cargo.toml")"
if [ "$workspace_revision" != "$LIBRAW_SOURCE_REVISION" ]; then
  echo "error: Cargo.toml pins LibRaw '$workspace_revision', but this script pins $LIBRAW_SOURCE_REVISION" >&2
  exit 1
fi

# The pinned master commit reports version 0.22.0, which is older than the
# 0.22.x releases, so this stamp rather than pkg-config identifies the source a
# cached prefix was built from.
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
