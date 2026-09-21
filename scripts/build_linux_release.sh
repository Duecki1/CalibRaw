#!/usr/bin/env bash
set -euxo pipefail

: "${RUSTUP_ARCH:?RUSTUP_ARCH must name the Rust host architecture}"

bootstrap_download_verified() {
  python3 scripts/bootstrap_download.py "$1" "$2" "$3"
}

bootstrap_download_verified \
  "https://static.rust-lang.org/rustup/archive/1.28.2/${RUSTUP_ARCH}-unknown-linux-gnu/rustup-init" \
  /tmp/rustup-init \
  "https://static.rust-lang.org/rustup/archive/1.28.2/${RUSTUP_ARCH}-unknown-linux-gnu/rustup-init.sha256"
chmod +x /tmp/rustup-init
/tmp/rustup-init -y --profile minimal --default-toolchain 1.92.0
source "$HOME/.cargo/env"

CALIBRAW_LIBRAW_REVISION="$(cargo metadata --locked --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["metadata"]["libraw_revision"])')"
LIBRAW_SOURCE_REVISION=b860248a89d9082b8e0a1e202e516f46af9adb29
LIBRAW_ARCHIVE_SHA256=f5da1e522ea195b54b30f3ff105ef2193daa04ea165dea825b4d6fe9d886395b
if [ -f "$LIBRAW_PREFIX/lib/pkgconfig/libraw.pc" ] \
  && [ -f "$LIBRAW_PREFIX/lib/libraw.so" ] \
  && [ "$(pkg-config --modversion libraw)" = "$CALIBRAW_LIBRAW_REVISION" ]; then
  echo "Using cached desktop LibRaw $LIBRAW_SOURCE_REVISION"
else
  rm -rf /tmp/calibraw-libraw-src "$LIBRAW_PREFIX"
  mkdir -p /tmp/calibraw-libraw-src
  python3 scripts/bootstrap_download.py \
    "https://github.com/LibRaw/LibRaw/archive/$LIBRAW_SOURCE_REVISION.tar.gz" \
    /tmp/calibraw-libraw.tar.gz "$LIBRAW_ARCHIVE_SHA256"
  tar -xzf /tmp/calibraw-libraw.tar.gz \
    --strip-components=1 \
    -C /tmp/calibraw-libraw-src
  (
    cd /tmp/calibraw-libraw-src
    autoreconf --install --force
    ./configure --prefix="$LIBRAW_PREFIX"
    make --jobs="$(nproc)"
    make install
  )
fi

LIBCLANG_SO="$(find /usr/lib -path '*/llvm-*/lib/libclang.so*' -print -quit 2>/dev/null || true)"
if [ -n "$LIBCLANG_SO" ]; then
  export LIBCLANG_PATH="$(dirname "$LIBCLANG_SO")"
fi

test "$(pkg-config --modversion libraw)" = "$CALIBRAW_LIBRAW_REVISION"
pkg-config --modversion lensfun
REVISION="$(git rev-parse --verify HEAD)"
export CALIBRAW_REQUIRE_COMMITTED_SOURCE=1
export CALIBRAW_SOURCE_REVISION="$REVISION"
export SOURCE_DATE_EPOCH="$(git show -s --format=%ct "$REVISION")"

cargo build --release --locked
strip target/release/calibraw

ldd target/release/calibraw | tee /tmp/calibraw-ldd.txt
grep -Eq 'libraw(_r)?\.so' /tmp/calibraw-ldd.txt
grep -Eq 'liblensfun\.so' /tmp/calibraw-ldd.txt
