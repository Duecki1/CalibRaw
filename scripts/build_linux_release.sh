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

bash scripts/build_libraw.sh "$LIBRAW_PREFIX"

LIBCLANG_SO="$(find /usr/lib -path '*/llvm-*/lib/libclang.so*' -print -quit 2>/dev/null || true)"
if [ -n "$LIBCLANG_SO" ]; then
  export LIBCLANG_PATH="$(dirname "$LIBCLANG_SO")"
fi

pkg-config --modversion libraw
cat "$LIBRAW_PREFIX/share/calibraw/libraw-revision"
pkg-config --modversion lensfun
REVISION="$(git rev-parse --verify HEAD)"
export CALIBRAW_REQUIRE_COMMITTED_SOURCE=1
export CALIBRAW_SOURCE_REVISION="$REVISION"
export SOURCE_DATE_EPOCH="$(git show -s --format=%ct "$REVISION")"

bash scripts/generate_licenses.sh
cargo build --release --locked
strip target/release/calibraw

ldd target/release/calibraw | tee /tmp/calibraw-ldd.txt
grep -Eq 'libraw(_r)?\.so' /tmp/calibraw-ldd.txt
grep -Eq 'liblensfun\.so' /tmp/calibraw-ldd.txt
