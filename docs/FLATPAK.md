# Flatpak

The Linux Flatpak app ID is `de.dueckis.CalibRaw`. Run the commands below from
the repository root. This workflow is local: nothing has been published to
Flathub, and none of these helpers push, upload, or submit anything.

## Setup and sources

Install your distribution's `flatpak`, Git, Python 3.11+ with venv/pip support,
and curl. Validation also needs `appstreamcli` and `desktop-file-validate`.
A working desktop portal with a file-picker backend is needed for folder
selection outside Pictures.

```sh
scripts/flatpak.sh setup
```

Setup adds Flathub for the current user and installs `org.flatpak.Builder`,
Freedesktop Platform/SDK 26.08, and the matching Rust stable and LLVM 22 SDK
extensions. The helper uses host `flatpak-builder` when available, otherwise
the Builder app. Initial SDK/source downloads and compilation are expensive,
especially the bundled ONNX Runtime; allow ample time and disk space.
Local builds use four parallel jobs by default; set `CALIBRAW_FLATPAK_JOBS`
to change this, for example `CALIBRAW_FLATPAK_JOBS=8 scripts/flatpak.sh build`.

Rust dependency sources are already described in
`packaging/flatpak/cargo-sources.json`. After changing `Cargo.lock`, regenerate
them before building:

```sh
scripts/flatpak.sh sources
```

This creates a local Python venv and downloads the pinned Cargo source
generator and dependencies. The manifest also pins LibRaw, Lensfun, and CPU
ONNX Runtime sources; compilation uses offline, locked Cargo dependencies
after the builder downloads the sources.

## Build, run, and debug

```sh
scripts/flatpak.sh build dev
scripts/flatpak.sh run
scripts/flatpak.sh debug
scripts/flatpak.sh shell
```

`build` defaults to `dev`. `prepare.py` copies the selected application source,
assets, and packaging files into `.flatpak/source`, including uncommitted edits
and eligible untracked, non-ignored files. It does not require a commit or
credentials. Credential/config directories such as `.aws`, `.git`, and your
home configuration are outside the snapshot's allowlist. The generated
manifest is `.flatpak/de.dueckis.CalibRaw.json`; build output, caches, and the
local OSTree repository also live under `.flatpak/`.
Each local profile keeps a Cargo target cache under `.flatpak/cargo-target/`
so editing the app does not discard dependency compilation. This cache mount
is excluded from the staged Flathub manifest.

The dev build still uses Cargo's release profile, but retains debug symbols,
sets Rust optimization to 1, disables LTO, uses 16 codegen units, and enables
incremental compilation. Use it for debugging rather than performance
measurements. `debug` starts GDB with
`/app/bin/calibraw` and maps project source paths to the read-only source
snapshot (enter `run` at the GDB prompt); `shell` opens an SDK shell
with the built application. `run` accepts additional application arguments.
`run`, `shell`, `debug`, and `smoke` install/update the user-scoped `local`
branch directly from `.flatpak/repo` before launching. `run` uses the Platform
runtime; `shell` and `debug` use the SDK. Normal Flatpak launching supplies the
filtered desktop portal bus and display access. Flatpak registers a
`calibraw-origin` remote pointing only to the local repository via `file://`.
Use `RUST_LOG=info scripts/flatpak.sh run` to enable application diagnostics.
Rebuild to include subsequent source edits.

```sh
scripts/flatpak.sh build release
scripts/flatpak.sh run
```

`release` uses the normal optimized profile with the same local snapshot and
`local` branch. It replaces the previous local build; it does not publish a
release. The package includes both `calibraw` and `calibraw-develop-export`.

## Sandbox and AI

Flatpak gives the app its own XDG config, data, and cache state under
`~/.var/app/de.dueckis.CalibRaw/`, separate from the native application's state.
Pictures has read/write access so `.calibraw` sidecars can be saved beside
their RAW files. Select other library folders through the folder portal to
grant access, and check sidecar saving in those locations too.

The manifest allows network access, Wayland with fallback X11, IPC sharing,
and DRI graphics devices for GPU rendering. It grants no general home access,
host CUDA access, or Discord IPC access by default.

CPU ONNX Runtime is bundled in `/app/lib`; the Flatpak does not need a runtime
download for AI. AI models remain optional downloads after user consent, and
inference runs locally on CPU. GPU photo rendering does not imply CUDA AI
support in this package.

The Open RAW action opens a folder picker in Flatpak so CalibRaw can read and
save adjacent sidecars; choose a photo from that folder in Library. Exports
keep the exact name selected in the save dialog, including names without an
extension. Deletion uses the host's Trash portal.

## Install and bundle

After building, install the current local branch or create a portable bundle:

```sh
scripts/flatpak.sh install
flatpak run de.dueckis.CalibRaw//local
scripts/flatpak.sh bundle
```

Installation is user-scoped and comes directly from `.flatpak/repo` through
its local `file://` origin. Run `install` again after rebuilding to update the installed
copy. The bundle is `dist/flatpak/de.dueckis.CalibRaw-local.flatpak` and can be
installed with:

```sh
flatpak install --user ./dist/flatpak/de.dueckis.CalibRaw-local.flatpak
```

Uninstall exactly the local branch with:

```sh
flatpak uninstall --user de.dueckis.CalibRaw//local
```

This command retains the application's state directory.

## Validation and manual checks

```sh
scripts/flatpak.sh validate
scripts/flatpak.sh smoke
```

Validation checks Cargo source coverage, AppStream metadata (with `--no-net
--pedantic`), the desktop file, the packaging manifest, and the build directory
if one exists. Builder's linter does not upload files. All checks run and the
command returns failure if any fails; a successful build alone does not
establish that validation or interactive checks passed.
Builds use Flathub's AppStream screenshot mirroring flags. The mirrored files
remain in the local build/repository; the helper does not upload them.

Known submission blocker: `dueckis.de` currently returns HTTP 525 from
Cloudflare, causing `appid-url-not-reachable`. The domain owner must fix its
HTTPS/origin configuration and rerun validation before future submission.
Keep the app ID `de.dueckis.CalibRaw`.

`smoke` runs on the actual Platform runtime with networking disabled. It probes
the bundled ONNX Runtime and renders a generated Bayer DNG through the packaged
CLI, checking the resulting PNG dimensions. Its synthetic input and output are
in `.flatpak/smoke`; it never reads your photos. This checks RAW decoding, GPU
export, and runtime loading without downloading an AI model.

Even if build and metadata checks pass, perform a full manual smoke check:

- Open a real RAW, edit it, save its adjacent sidecar, close and reopen the app,
  and confirm the edits persist. Repeat in Pictures and a portal-selected folder.
- Export the edited RAW and open the output to check the rendered result.
- Export to a new name without an extension and to an existing filename;
  verify the selected file persists after closing the app.
- Move a disposable test RAW and its sidecar to Trash and restore both from
  your desktop's trash folder.
- Exercise an AI tool: check download consent, accept a model download, and
  confirm CPU inference produces a usable result with the bundled runtime.

Record the actual results and any failures before treating a build as tested.

## Future Flathub staging

Commit the tested application and packaging changes before staging a specific
commit or tag:

```sh
scripts/flatpak.sh stage-release COMMIT_OR_TAG
```

The helper checks source coverage and compares selected required files with
the requested commit, then creates `dist/flatpak/flathub/<full-commit>/`. It
refuses to overwrite an existing staging directory. The staged manifest drops
the `local` default branch and pins the GitHub source to that commit, alongside
the Cargo and ONNX source files. Review all staged files and verify that the
entire tested tree matches the commit; the helper compares application sources,
assets, and required metadata with the requested commit.

Staging remains unpublished. The pinned commit must be publicly fetchable for
others to build it. Resolve the domain/linter blocker, validate and build the
staged manifest, and complete the manual checks before a separate future
Flathub submission.
