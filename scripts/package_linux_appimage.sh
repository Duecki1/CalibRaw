#!/usr/bin/env bash
set -euxo pipefail

: "${LINUXDEPLOY_ARCH:?LINUXDEPLOY_ARCH must name the linuxdeploy architecture}"
: "${LINUXDEPLOY_SHA256:?LINUXDEPLOY_SHA256 must contain the linuxdeploy digest}"

source "$HOME/.cargo/env"

LIBRAW_SO="$(ldd target/release/calibraw | awk '/libraw(_r)?\.so/{print $3; exit}')"
LENSFUN_SO="$(ldd target/release/calibraw | awk '/liblensfun\.so/{print $3; exit}')"
test -f "$LIBRAW_SO"
test -f "$LENSFUN_SO"
echo "Bundling LibRaw from $LIBRAW_SO"
echo "Bundling Lensfun from $LENSFUN_SO"

LENSFUN_DB="$(find /usr/share/lensfun -type f -name '*.xml' -printf '%h\n' | head -n 1)"
test -n "$LENSFUN_DB"

rm -rf AppDir dist appimage-packaging
mkdir -p dist appimage-packaging AppDir/usr/share/calibraw/lensfun AppDir/usr/share/doc/calibraw
install -m 0644 COPYING THIRD_PARTY_NOTICES.md THIRD_PARTY_LICENSES.md \
  AppDir/usr/share/doc/calibraw/
cp -a "$LENSFUN_DB"/. AppDir/usr/share/calibraw/lensfun/
test -n "$(find AppDir/usr/share/calibraw/lensfun -name '*.xml' -print -quit)"

APP_ID=de.duecki.calibraw
DESKTOP_FILE="$PWD/packaging/linux/$APP_ID.desktop"
APPIMAGE_ICON="$PWD/appimage-packaging/$APP_ID.png"
test -s "$DESKTOP_FILE"
grep -qx "Icon=$APP_ID" "$DESKTOP_FILE"
grep -qx "StartupWMClass=$APP_ID" "$DESKTOP_FILE"

install -m 0644 packaging/icons/calibraw-256.png "$APPIMAGE_ICON"
install -m 0644 "$DESKTOP_FILE" "AppDir/$APP_ID.desktop"
install -Dm 0644 "$DESKTOP_FILE" "AppDir/usr/share/applications/$APP_ID.desktop"
install -m 0644 "$APPIMAGE_ICON" "AppDir/$APP_ID.png"
install -Dm 0644 "$APPIMAGE_ICON" "AppDir/usr/share/icons/hicolor/256x256/apps/$APP_ID.png"
ln -s "$APP_ID.png" AppDir/.DirIcon
test -s "$APPIMAGE_ICON"
test -L AppDir/.DirIcon

LINUXDEPLOY=/tmp/linuxdeploy-${LINUXDEPLOY_ARCH}.AppImage
python3 scripts/bootstrap_download.py \
  "https://github.com/linuxdeploy/linuxdeploy/releases/download/1-alpha-20251107-1/linuxdeploy-${LINUXDEPLOY_ARCH}.AppImage" \
  "$LINUXDEPLOY" "$LINUXDEPLOY_SHA256"
chmod +x "$LINUXDEPLOY"

export APPIMAGE_EXTRACT_AND_RUN=1
export LDAI_OUTPUT="CalibRaw-${LINUXDEPLOY_ARCH}.AppImage"
"$LINUXDEPLOY" \
  --appdir AppDir \
  --executable "$PWD/target/release/calibraw" \
  --library "$LIBRAW_SO" \
  --library "$LENSFUN_SO" \
  --desktop-file "$DESKTOP_FILE" \
  --icon-file "$APPIMAGE_ICON" \
  --output appimage

# The AppImage must run on the oldest supported Ubuntu LTS (22.04, glibc 2.35).
# Fail if any bundled ELF file needs a newer glibc, e.g. after a runner upgrade.
MAX_GLIBC=2.35
newest_glibc="$(find AppDir -type f -exec objdump -T {} + 2>/dev/null \
  | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -Vu | tail -n 1)"
echo "Newest glibc symbol required: ${newest_glibc:-none}"
test "$(printf '%s\n%s\n' "$MAX_GLIBC" "${newest_glibc:-0}" | sort -V | tail -n 1)" = "$MAX_GLIBC"

mv "$LDAI_OUTPUT" dist/
chmod +x "dist/$LDAI_OUTPUT"
file "dist/$LDAI_OUTPUT"
sha256sum "dist/$LDAI_OUTPUT" > "dist/$LDAI_OUTPUT.sha256"
