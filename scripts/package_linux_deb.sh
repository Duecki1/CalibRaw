#!/usr/bin/env bash
# Packages the binary from scripts/build_linux_release.sh as
# dist/CalibRaw-<architecture>.deb. One package per architecture installs on
# Debian and Ubuntu, so it may depend only on packages that both ship.
set -euxo pipefail

: "${LENSFUN_PREFIX:?LENSFUN_PREFIX must name the Lensfun install prefix}"

VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n 1)"
ARCH="$(dpkg --print-architecture)"
test -n "$VERSION"

LIBRAW_SO="$(ldd target/release/calibraw | awk '/libraw(_r)?\.so/{print $3; exit}')"
LENSFUN_SO="$(ldd target/release/calibraw | awk '/liblensfun\.so/{print $3; exit}')"
# Ubuntu's libjpeg.so.8 has no Debian package; Debian ships libjpeg.so.62.
LIBJPEG_SO="$(ldd "$LIBRAW_SO" | awk '/libjpeg\.so/{print $3; exit}')"
test -f "$LIBRAW_SO"
test -f "$LENSFUN_SO"
test -f "$LIBJPEG_SO"

LENSFUN_DB="$(find "$LENSFUN_PREFIX/share/lensfun" -type f -name '*.xml' -printf '%h\n' | head -n 1)"
test -n "$LENSFUN_DB"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# The pinned LibRaw and Lensfun stay in a private directory so they cannot
# collide with the distribution's older builds. database_candidates() finds the
# Lensfun database relative to /usr/bin.
APP_ID=de.duecki.calibraw
root="$work/debian/calibraw"
private_libs="$root/usr/lib/calibraw"
mkdir -p "$private_libs" "$root/usr/share/calibraw/lensfun" "$root/DEBIAN"

install -Dm 0755 target/release/calibraw "$root/usr/bin/calibraw"
patchelf --set-rpath '$ORIGIN/../lib/calibraw' "$root/usr/bin/calibraw"
for library in "$LIBRAW_SO" "$LENSFUN_SO" "$LIBJPEG_SO"; do
  installed="$private_libs/$(basename "$library")"
  install -m 0644 "$library" "$installed"
  strip --strip-unneeded "$installed"
  patchelf --set-rpath '$ORIGIN' "$installed"
done

cp -a "$LENSFUN_DB"/. "$root/usr/share/calibraw/lensfun/"
test -n "$(find "$root/usr/share/calibraw/lensfun" -name '*.xml' -print -quit)"

install -Dm 0644 "packaging/linux/$APP_ID.desktop" "$root/usr/share/applications/$APP_ID.desktop"
install -Dm 0644 packaging/icons/calibraw-256.png "$root/usr/share/icons/hicolor/256x256/apps/$APP_ID.png"
install -Dm 0644 COPYING "$root/usr/share/doc/calibraw/copyright"
install -m 0644 THIRD_PARTY_NOTICES.md THIRD_PARTY_LICENSES.md "$root/usr/share/doc/calibraw/"
chmod -R u=rwX,go=rX "$root"

# dpkg-shlibdeps only needs debian/control to exist. The private libraries have
# no shlibs information, and the workflow's LD_LIBRARY_PATH would find the build
# prefixes instead of the private directory.
touch "$work/debian/control"
SHLIBS="$(cd "$work" && env -u LD_LIBRARY_PATH dpkg-shlibdeps -O --ignore-missing-info \
  "-l$private_libs" "$root/usr/bin/calibraw" "$private_libs"/* \
  | sed -n 's/^shlibs:Depends=//p')"
test -n "$SHLIBS"

# The binary opens these at run time, so dpkg-shlibdeps cannot see them.
RUNTIME_LIBS="libdbus-1-3, libegl1, libvulkan1, libwayland-client0, libwayland-egl1, libx11-6, libx11-xcb1, libxcursor1, libxi6, libxkbcommon0, libxkbcommon-x11-0"

cat > "$root/DEBIAN/control" <<EOF
Package: calibraw
Version: $VERSION
Section: graphics
Priority: optional
Architecture: $ARCH
Maintainer: Duecki <calibraw@dueckis.de>
Installed-Size: $(du -sk --exclude=DEBIAN "$root" | cut -f1)
Depends: $SHLIBS, $RUNTIME_LIBS
Recommends: mesa-vulkan-drivers | vulkan-icd, xdg-desktop-portal
Homepage: https://calibraw.dueckis.de
Description: GPU-first RAW image editor
 CalibRaw edits RAW photos on the GPU through wgpu, with LibRaw for decoding
 and Lensfun for lens correction.
EOF

# xz because older dpkg versions cannot read Ubuntu's default zstd.
OUTPUT="CalibRaw-$ARCH.deb"
mkdir -p dist
dpkg-deb -Zxz --root-owner-group --build "$root" "dist/$OUTPUT"
dpkg-deb --info "dist/$OUTPUT"
sha256sum "dist/$OUTPUT" > "dist/$OUTPUT.sha256"
