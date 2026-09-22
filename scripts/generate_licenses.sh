#!/usr/bin/env bash
set -euo pipefail

cargo_about_version="0.9.2"
if ! command -v cargo-about >/dev/null 2>&1 \
    || [ "$(cargo about --version 2>/dev/null || true)" != "cargo-about ${cargo_about_version}" ]; then
  cargo install cargo-about --version "$cargo_about_version" --locked
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
