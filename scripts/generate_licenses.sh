#!/usr/bin/env bash
set -euo pipefail

cargo_about_version="0.9.2"
if ! command -v cargo-about >/dev/null 2>&1 \
    || [ "$(cargo about --version 2>/dev/null || true)" != "cargo-about ${cargo_about_version}" ]; then
  cargo_about_install_args=(
    --version "$cargo_about_version"
    --locked
    --features cli
  )
  if [ "${MSYSTEM:-}" = "CLANGARM64" ]; then
    cargo_about_install_args+=(--target aarch64-pc-windows-gnullvm)
  fi
  cargo install cargo-about "${cargo_about_install_args[@]}"
fi

output=${1:-THIRD_PARTY_LICENSES.md}
temporary=$(mktemp "${TMPDIR:-/tmp}/calibraw-licenses.XXXXXX")
trap 'rm -f "$temporary"' EXIT

# cargo-about has emitted CRLF in otherwise identical output on some hosts.
# Normalize only line endings so the checked-in notice is reproducible without
# changing the license text itself.
LC_ALL=C cargo about generate --locked --workspace --fail \
  -o "$temporary" about.hbs
sed 's/\r$//' "$temporary" > "$output"
