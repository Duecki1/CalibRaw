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
rust_version="$(sed -n 's/^channel = "\(.*\)"$/\1/p' rust-toolchain.toml)"
/tmp/rustup-init -y --profile minimal --default-toolchain "$rust_version"
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

# Build the same pinned Lensfun source as Android instead of using the distro
# package: Ubuntu 22.04 ships Lensfun 0.3.2 with an older lens database.
CALIBRAW_LENSFUN_REVISION="$(cargo metadata --locked --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["metadata"]["lensfun_revision"])')"
LENSFUN_SOURCE_REVISION=101c745e847a5de4a1e569a94368ce2027198598
LENSFUN_ARCHIVE_SHA256=a11cbe6aeec657839540448b253217c25d20b7a45b6aebfef406f7239933c7a6
if [ -f "$LENSFUN_PREFIX/lib/pkgconfig/lensfun.pc" ] \
  && [ -f "$LENSFUN_PREFIX/lib/liblensfun.so" ] \
  && [ -f "$LENSFUN_PREFIX/share/lensfun/version_1/timestamp.txt" ] \
  && [ "$(pkg-config --modversion lensfun)" = "$CALIBRAW_LENSFUN_REVISION.0" ]; then
  echo "Using cached desktop Lensfun $LENSFUN_SOURCE_REVISION"
else
  rm -rf /tmp/calibraw-lensfun-src /tmp/calibraw-lensfun-build "$LENSFUN_PREFIX"
  mkdir -p /tmp/calibraw-lensfun-src
  python3 scripts/bootstrap_download.py \
    "https://github.com/lensfun/lensfun/archive/$LENSFUN_SOURCE_REVISION.tar.gz" \
    /tmp/calibraw-lensfun.tar.gz "$LENSFUN_ARCHIVE_SHA256"
  tar -xzf /tmp/calibraw-lensfun.tar.gz \
    --strip-components=1 \
    -C /tmp/calibraw-lensfun-src
  # PYTHON=OFF skips the helper-script Python package, which is always built
  # when python3 is found and would need setuptools. Lensfun 0.3.4 declares
  # CMake 2.8.12, which CMake 4 only accepts with a policy minimum.
  cmake -S /tmp/calibraw-lensfun-src -B /tmp/calibraw-lensfun-build \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_POLICY_VERSION_MINIMUM=3.5 \
    -DCMAKE_INSTALL_PREFIX="$LENSFUN_PREFIX" \
    -DCMAKE_INSTALL_LIBDIR=lib \
    -DBUILD_TESTS=OFF \
    -DBUILD_LENSTOOL=OFF \
    -DBUILD_DOC=OFF \
    -DINSTALL_PYTHON_MODULE=OFF \
    -DINSTALL_HELPER_SCRIPTS=OFF \
    -DPYTHON=OFF
  cmake --build /tmp/calibraw-lensfun-build --target install --parallel "$(nproc)"
fi

LIBCLANG_SO="$(find /usr/lib -path '*/llvm-*/lib/libclang.so*' -print -quit 2>/dev/null || true)"
if [ -n "$LIBCLANG_SO" ]; then
  export LIBCLANG_PATH="$(dirname "$LIBCLANG_SO")"
fi

test "$(pkg-config --modversion libraw)" = "$CALIBRAW_LIBRAW_REVISION"
test "$(pkg-config --modversion lensfun)" = "$CALIBRAW_LENSFUN_REVISION.0"
REVISION="$(git rev-parse --verify HEAD)"
export CALIBRAW_REQUIRE_COMMITTED_SOURCE=1
export CALIBRAW_SOURCE_REVISION="$REVISION"
export SOURCE_DATE_EPOCH="$(git show -s --format=%ct "$REVISION")"

bash scripts/generate_licenses.sh
cargo build --release --locked
strip target/release/calibraw

ldd target/release/calibraw | tee /tmp/calibraw-ldd.txt
grep -Eq 'libraw(_r)?\.so' /tmp/calibraw-ldd.txt
grep -Eq "liblensfun\\.so.* => $LENSFUN_PREFIX/lib/" /tmp/calibraw-ldd.txt
