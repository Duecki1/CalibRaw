#!/usr/bin/env bash
# Build only the validator. Ubuntu 24.04's spirv-tools misses VUID-10684.
set -euo pipefail

output="${1:?usage: build_spirv_val.sh OUTPUT_DIRECTORY}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
download="$(dirname "$0")/bootstrap_download.py"

unpack() {
  local repo="$1" revision="$2" sha256="$3" dest="$4"
  python3 "$download" "https://codeload.github.com/KhronosGroup/$repo/tar.gz/$revision" \
    "$work/$repo.tar.gz" "$sha256"
  mkdir -p "$dest"
  tar -xzf "$work/$repo.tar.gz" --strip-components=1 -C "$dest"
}

# SPIRV-Tools v2026.4 and the SPIRV-Headers revision in its DEPS.
unpack SPIRV-Tools ef96ed763b43b59b33b31b362f09a02b729fa1c9 \
  82c62146083fd558735a3171cf97cfc47903ca7d368482e87f94bd44883c0f00 "$work/source"
unpack SPIRV-Headers 496543121ce6419f23d6fa5d7194ba66c36212d2 \
  a9bb9c48713245eacf97cc539b6f1d45405a92d8813f8b82e635f0a085ee9898 "$work/source/external/spirv-headers"

cmake -S "$work/source" -B "$work/build" -DCMAKE_BUILD_TYPE=Release -DSPIRV_SKIP_TESTS=ON
cmake --build "$work/build" --target spirv-val --parallel "$(nproc)"
install -Dm755 "$work/build/tools/spirv-val" "$output/spirv-val"
