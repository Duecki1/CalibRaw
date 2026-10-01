#!/usr/bin/env python3
"""Prepare a local source snapshot or an unpublished Flathub submission directory."""

import argparse
import json
import os
import shutil
import subprocess
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PACKAGING = ROOT / "packaging/flatpak"
APP_ID = "de.dueckis.CalibRaw"


def git(*args: str) -> bytes:
    return subprocess.check_output(["git", "-C", str(ROOT), *args], stderr=subprocess.PIPE)


def check_sources() -> None:
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text())
    sources = json.loads((PACKAGING / "cargo-sources.json").read_text())
    archives = {source.get("sha256") for source in sources if source["type"] == "archive"}
    commits = {source.get("commit") for source in sources if source["type"] == "git"}
    for package in lock["package"]:
        source = package.get("source", "")
        if source.startswith("registry+") and package["checksum"] not in archives:
            raise SystemExit("Cargo.lock changed; run scripts/flatpak.sh sources first")
        if source.startswith("git+") and source.rsplit("#", 1)[1] not in commits:
            raise SystemExit("Git dependency changed; run scripts/flatpak.sh sources first")


def snapshot(profile: str) -> None:
    check_sources()
    source = ROOT / ".flatpak/source"
    if source.exists():
        shutil.rmtree(source)
    source.mkdir(parents=True)
    root_files = {
        "Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "README.md", "COPYING",
        "THIRD_PARTY_NOTICES.md", ".cargo/config.toml",
    }
    prefixes = ("crates/", "xtask/", "data/", "packaging/icons/", "packaging/linux/", "packaging/flatpak/")
    paths = git("ls-files", "-z", "--cached", "--others", "--exclude-standard").split(b"\0")
    for raw in paths:
        if not raw:
            continue
        relative = raw.decode()
        original = ROOT / relative
        if not original.is_file() or (relative not in root_files and not relative.startswith(prefixes)):
            continue
        destination = source / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(original, destination)
    manifest = json.loads((PACKAGING / f"{APP_ID}.json").read_text())
    # Keep the source paths valid after moving the prepared manifest to .flatpak.
    manifest["modules"][2] = "../packaging/flatpak/onnxruntime-module.json"
    manifest["modules"][-1]["sources"] = [
        {"type": "dir", "path": "source"},
        "../packaging/flatpak/cargo-sources.json",
    ]
    environment = manifest["build-options"]["env"]
    environment["CALIBRAW_SOURCE_REVISION"] = git("rev-parse", "HEAD").decode().strip() + "-local"
    # Preserve compilation results when flatpak-builder replaces the app's
    # source directory after an edit. This mount exists only in local builds.
    target = ROOT / ".flatpak/cargo-target" / profile
    target.mkdir(parents=True, exist_ok=True)
    environment["CARGO_TARGET_DIR"] = str(target)
    environment["CARGO_BUILD_JOBS"] = os.environ.get("CALIBRAW_FLATPAK_JOBS", "4")
    manifest["build-options"]["build-args"] = [f"--filesystem={target}"]
    if profile == "dev":
        environment.update({
            "CARGO_PROFILE_RELEASE_DEBUG": "2",
            "CARGO_PROFILE_RELEASE_LTO": "false",
            "CARGO_PROFILE_RELEASE_CODEGEN_UNITS": "16",
            "CARGO_PROFILE_RELEASE_OPT_LEVEL": "1",
            "CARGO_PROFILE_RELEASE_INCREMENTAL": "true",
        })
        manifest["build-options"]["strip"] = False
        manifest["build-options"]["no-debuginfo"] = True
    output = ROOT / f".flatpak/{APP_ID}.json"
    output.write_text(json.dumps(manifest, indent=4) + "\n")
    (ROOT / ".flatpak/profile").write_text(profile + "\n")
    print(f"Prepared {profile} build from current working tree: {output}")


def stage_release(revision: str) -> None:
    check_sources()
    try:
        commit = git("rev-parse", "--verify", revision + "^{commit}").decode().strip()
    except subprocess.CalledProcessError:
        raise SystemExit(f"Not a valid committed revision: {revision}") from None
    required = [
        "Cargo.toml", "Cargo.lock", "packaging/linux/de.dueckis.CalibRaw.metainfo.xml",
        "packaging/flatpak/generate-licenses.py", "crates/calibraw-ai/src/onnx_runtime_artifact.rs",
    ]
    for filename in required:
        try:
            committed = git("show", f"{commit}:{filename}")
        except subprocess.CalledProcessError:
            raise SystemExit(f"{filename} is absent from {commit}; commit the tested packaging first") from None
        if committed != (ROOT / filename).read_bytes():
            raise SystemExit(f"{filename} differs from {commit}; commit the tested packaging first")
    differences = git("diff", "--name-only", commit, "--", "Cargo.toml", "Cargo.lock", "crates", "xtask", "data", "packaging/linux", "packaging/icons", "packaging/flatpak/generate-licenses.py")
    if differences.strip():
        raise SystemExit("Application files differ from the requested commit:\n" + differences.decode())
    # Do not silently overwrite a directory the developer might be reviewing.
    output = ROOT / "dist/flatpak/flathub" / commit
    output.mkdir(parents=True, exist_ok=False)
    manifest = json.loads((PACKAGING / f"{APP_ID}.json").read_text())
    manifest.pop("default-branch", None)
    manifest["build-options"]["env"]["CALIBRAW_SOURCE_REVISION"] = commit
    manifest["build-options"]["env"]["SOURCE_DATE_EPOCH"] = git("show", "-s", "--format=%ct", commit).decode().strip()
    manifest["modules"][-1]["sources"] = [
        {"type": "git", "url": "https://github.com/Duecki1/CalibRaw.git", "commit": commit},
        "cargo-sources.json",
    ]
    (output / f"{APP_ID}.json").write_text(json.dumps(manifest, indent=4) + "\n")
    for filename in ("cargo-sources.json", "onnxruntime-module.json", "onnxruntime-sources.json", "generate-onnx-sources.py"):
        shutil.copy2(PACKAGING / filename, output / filename)
    print(f"Staged unpublished release files: {output}")
    print("The pinned commit must be publicly fetchable before anyone can build these files.")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    local = sub.add_parser("snapshot")
    local.add_argument("profile", choices=["dev", "release"])
    release = sub.add_parser("stage-release")
    release.add_argument("revision")
    sub.add_parser("check-sources")
    args = parser.parse_args()
    if args.command == "snapshot":
        snapshot(args.profile)
    elif args.command == "stage-release":
        stage_release(args.revision)
    else:
        check_sources()


if __name__ == "__main__":
    main()
