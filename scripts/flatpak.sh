#!/usr/bin/env bash
set -euo pipefail

root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
app_id=de.dueckis.CalibRaw
manifest="$root/.flatpak/$app_id.json"
build_dir="$root/.flatpak/build"
repo="$root/.flatpak/repo"
builder_tools_commit=74697c75b630d7330e77250fc13cb5ea688d9479

builder() {
  if command -v flatpak-builder >/dev/null 2>&1; then
    flatpak-builder "$@"
  else
    flatpak run org.flatpak.Builder "$@"
  fi
}

install_local() {
  # Use the installed local branch for the real application portal sandbox.
  flatpak install --user --noninteractive --reinstall "$repo" "$app_id//local"
}

require_build() {
  if [[ ! -x "$build_dir/files/bin/calibraw" || ! -f "$build_dir/metadata" || ! -f "$manifest" ]]; then
    echo "No local Flatpak build. Run scripts/flatpak.sh build first." >&2
    exit 1
  fi
}

case "${1:-help}" in
  setup)
    flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
    flatpak install --user --noninteractive flathub \
      org.flatpak.Builder org.freedesktop.Platform//26.08 org.freedesktop.Sdk//26.08 \
      org.freedesktop.Sdk.Extension.rust-stable//26.08 org.freedesktop.Sdk.Extension.llvm22//26.08
    ;;
  sources)
    mkdir -p .flatpak/tools
    python3 -m venv .flatpak/tools/venv
    .flatpak/tools/venv/bin/pip install 'aiohttp>=3.9.5,<4' 'tomlkit>=0.13.3,<1'
    curl --fail --location \
      "https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/$builder_tools_commit/cargo/flatpak-cargo-generator.py" \
      --output .flatpak/tools/flatpak-cargo-generator.py
    .flatpak/tools/venv/bin/python .flatpak/tools/flatpak-cargo-generator.py \
      Cargo.lock -o packaging/flatpak/cargo-sources.json
    python3 packaging/flatpak/prepare.py check-sources
    ;;
  build)
    profile="${2:-dev}"
    python3 packaging/flatpak/prepare.py snapshot "$profile"
    builder --user --force-clean --ccache --jobs="${CALIBRAW_FLATPAK_JOBS:-4}" --state-dir="$root/.flatpak/builder" \
      --compose-url-policy=full --mirror-screenshots-url=https://dl.flathub.org/media \
      --repo="$repo" "$build_dir" "$manifest"
    ;;
  run)
    require_build
    shift
    install_local
    exec flatpak run --user --branch=local "$app_id" "$@"
    ;;
  shell)
    require_build
    install_local
    exec flatpak run --user --branch=local --devel --command=sh "$app_id"
    ;;
  debug)
    require_build
    install_local
    exec flatpak run --user --branch=local --devel --filesystem="$root/.flatpak/source:ro" \
      --command=gdb "$app_id" \
      -ex "set substitute-path /run/build/calibraw \"$root/.flatpak/source\"" /app/bin/calibraw
    ;;
  install)
    require_build
    install_local
    ;;
  bundle)
    require_build
    mkdir -p dist/flatpak
    flatpak build-bundle "$repo" "dist/flatpak/$app_id-local.flatpak" "$app_id" local
    ;;
  smoke)
    require_build
    install_local
    python3 packaging/flatpak/smoke-test.py
    ;;
  validate)
    validation_status=0
    python3 packaging/flatpak/prepare.py check-sources || validation_status=1
    appstreamcli validate --no-net --pedantic packaging/linux/$app_id.metainfo.xml || validation_status=1
    desktop-file-validate packaging/linux/$app_id.desktop || validation_status=1
    # Builder's linter is local and does not upload files.
    flatpak run --command=flatpak-builder-lint org.flatpak.Builder manifest packaging/flatpak/$app_id.json || validation_status=1
    if [[ -f "$build_dir/metadata" ]]; then
      flatpak run --command=flatpak-builder-lint org.flatpak.Builder builddir "$build_dir" || validation_status=1
    fi
    exit "$validation_status"
    ;;
  stage-release)
    [[ $# -eq 2 ]] || { echo "Usage: scripts/flatpak.sh stage-release COMMIT_OR_TAG" >&2; exit 2; }
    python3 packaging/flatpak/prepare.py stage-release "$2"
    ;;
  help|--help|-h)
    cat <<'EOF'
Usage: scripts/flatpak.sh COMMAND

  setup                  Install user-scoped Flatpak build tools and SDKs
  sources                Regenerate offline Rust sources after Cargo.lock changes
  build [dev|release]    Build current working tree (default: dev, with debug symbols)
  run [ARGS...]          Install/update the local branch and run it in its sandbox
  shell                  Open a shell with the SDK and local application
  debug                  Start the local application under GDB
  install                Install the local branch for the current user
  bundle                 Create dist/flatpak/de.dueckis.CalibRaw-local.flatpak
  smoke                  Test offline runtime loading and a synthetic RAW GPU export
  validate               Check sources, metadata, manifest, and any existing build
  stage-release COMMIT   Stage unpublished files pinned to a tested commit

Nothing is pushed, uploaded, or submitted to Flathub.
EOF
    ;;
  *)
    echo "Unknown command: $1. Run scripts/flatpak.sh help." >&2
    exit 2
    ;;
esac
