#!/usr/bin/env bash
# Builds the pinned Lensfun commit into PREFIX for the desktop builds (CI,
# Linux AppImage, Windows, macOS). The last Lensfun release, 0.3.4 from 2023,
# lacks the lens profiles added since, so every platform builds this master
# commit and bundles its database. Extra arguments go to CMake.
#
# Usage: scripts/build_lensfun.sh PREFIX [CMAKE_ARG...]
set -euo pipefail

# Keep in sync with workspace.metadata.lensfun_revision in Cargo.toml, the
# Android pin in android/app/src/main/cpp/CMakeLists.txt and the Flatpak
# manifest.
LENSFUN_SOURCE_REVISION=bc5615d6278cb5fb75dd53e74015367453d5591d
LENSFUN_ARCHIVE_SHA256=b56bff238bd9b8f6e517785de9361a11608f0d2afbfa9e21ce4a33d69d5c2046

prefix="${1:?usage: scripts/build_lensfun.sh PREFIX [CMAKE_ARG...]}"
shift

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
workspace_revision="$(sed -n 's/^lensfun_revision = "\(.*\)"$/\1/p' "$repository_root/Cargo.toml")"
if [ "$workspace_revision" != "$LENSFUN_SOURCE_REVISION" ]; then
  echo "error: Cargo.toml pins Lensfun '$workspace_revision', but this script pins $LENSFUN_SOURCE_REVISION" >&2
  exit 1
fi

# Every master commit reports version 0.3.99, so this stamp rather than
# pkg-config identifies the source a cached prefix was built from.
stamp="$prefix/share/calibraw/lensfun-revision"
if [ -f "$prefix/lib/pkgconfig/lensfun.pc" ] \
  && [ "$(cat "$stamp" 2>/dev/null || true)" = "$LENSFUN_SOURCE_REVISION" ]; then
  echo "Using cached Lensfun $LENSFUN_SOURCE_REVISION from $prefix"
  exit 0
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

python3 "$repository_root/scripts/bootstrap_download.py" \
  "https://github.com/lensfun/lensfun/archive/$LENSFUN_SOURCE_REVISION.tar.gz" \
  "$work/lensfun.tar.gz" "$LENSFUN_ARCHIVE_SHA256"
mkdir "$work/source"
tar -xzf "$work/lensfun.tar.gz" --strip-components=1 -C "$work/source"

# The database installs to PREFIX/share/lensfun/version_2. An explicit data
# root keeps Windows from installing it under %ALLUSERSPROFILE%, and the
# absolute install name lets macOS find the library without an rpath, like
# LibRaw's.
jobs="$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 2)"
cmake -S "$work/source" -B "$work/build" \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX="$prefix" \
  -DCMAKE_INSTALL_LIBDIR=lib \
  -DCMAKE_INSTALL_DATAROOTDIR=share \
  -DCMAKE_INSTALL_NAME_DIR="$prefix/lib" \
  -DBUILD_STATIC=OFF \
  -DBUILD_TESTS=OFF \
  -DBUILD_LENSTOOL=OFF \
  -DBUILD_DOC=OFF \
  -DINSTALL_PYTHON_MODULE=OFF \
  -DINSTALL_HELPER_SCRIPTS=OFF \
  "$@"
cmake --build "$work/build" --parallel "$jobs"
cmake --install "$work/build"

mkdir -p "$(dirname "$stamp")"
printf '%s\n' "$LENSFUN_SOURCE_REVISION" > "$stamp"
echo "Installed Lensfun $LENSFUN_SOURCE_REVISION into $prefix"
