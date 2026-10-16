#!/usr/bin/env python3
"""Build the Automatic ONNX Runtime archives with the WebGPU execution provider.

Usage: python3 packaging/onnxruntime/package_webgpu.py OUTPUT_DIR [DOWNLOAD_DIR]

Each archive holds Microsoft's ONNX Runtime core library for one platform and
Microsoft's WebGPU plugin execution provider (`onnxruntime-ep-webgpu` on PyPI)
beside it in lib/, both unmodified, with their licenses. CalibRaw loads the
core and registers the plugin from the same folder. On Windows the plugin's
dxcompiler.dll and dxil.dll stay beside it, where Dawn looks for them.

The plugin needs ONNX Runtime 1.24.4 or newer and exists for these five
platforms only; macOS x86_64 keeps its CPU-only runtime.

Downloads are SHA-256 pinned and kept in DOWNLOAD_DIR when it is given. The
output is byte-reproducible with the same Python and zlib. Upload each archive
unchanged to the CalibRaw-Artifacts repository at onnxruntime/<platform>/, the
path it is written to under OUTPUT_DIR, and copy the printed size and SHA-256
into crates/calibraw-ai/src/runtime/package.rs.
"""

import gzip
import hashlib
import io
import sys
import tarfile
import urllib.request
import zipfile
from pathlib import Path

ORT_VERSION = "1.30.0"
WEBGPU_VERSION = "0.4.0"
ORT_RELEASE = f"https://github.com/microsoft/onnxruntime/releases/download/v{ORT_VERSION}"
WEBGPU_WHEELS = "https://files.pythonhosted.org/packages"

# CalibRaw platform -> core archive, its SHA-256, the core library in it,
# plugin wheel URL path, its SHA-256, and the plugin's files.
PLATFORMS = {
    "linux-x86_64": (
        f"onnxruntime-linux-x64-{ORT_VERSION}.tgz",
        "a5ed5a3cac51fbb2e90da632ae43d19212faaa20e76484e62bcb7c23ddb3b3fd",
        f"lib/libonnxruntime.so.{ORT_VERSION}",
        "c1/96/2a18a45079250afcd825687aef2895de266d5abc1cff9f52d2dede7598f2/"
        f"onnxruntime_ep_webgpu-{WEBGPU_VERSION}-py3-none-manylinux_2_28_x86_64.whl",
        "5d8c961eda91a88961cad1fb7621f389f006b18497d6ea9ecf96f729f066cbfb",
        ["libonnxruntime_providers_webgpu.so"],
    ),
    "linux-arm64": (
        f"onnxruntime-linux-aarch64-{ORT_VERSION}.tgz",
        "e16a27a8ed330bbc698df7330b0cf56e722f354e3bcc92118682c74ef3c3e3da",
        f"lib/libonnxruntime.so.{ORT_VERSION}",
        "4e/ca/00c70322c19913c81a6bb2239aca83781b97a818b55c2ec3d25cec72dc4c/"
        f"onnxruntime_ep_webgpu-{WEBGPU_VERSION}-py3-none-manylinux_2_28_aarch64.whl",
        "17f660db53b1a509c63e721ac6784a2daa1c4e91a968523ee9986505ee5b0a55",
        ["libonnxruntime_providers_webgpu.so"],
    ),
    # The wheel is tagged universal2, but its library is arm64 only.
    "macos-arm64": (
        f"onnxruntime-osx-arm64-{ORT_VERSION}.tgz",
        "6ebb5062a934537c352937821f9fe9718e7de1a2db1122a93dd363ffd53a7012",
        f"lib/libonnxruntime.{ORT_VERSION}.dylib",
        "c3/9f/1e09865bbe4c9202ff1334545535be32dfdbeffc46e7d4c36c5de67ffafe/"
        f"onnxruntime_ep_webgpu-{WEBGPU_VERSION}-py3-none-macosx_14_0_universal2.whl",
        "8d0a91d44b43d931c3068b9134aed9770f27803e353c2b58c93e1700bb68bcca",
        ["libonnxruntime_providers_webgpu.dylib"],
    ),
    "windows-x86_64": (
        f"onnxruntime-win-x64-{ORT_VERSION}.zip",
        "c6ba983baf5681af108599675d2a89c2d145512d02de28aed0bff177cd0ba949",
        "lib/onnxruntime.dll",
        "d7/a4/c98a9e9433b3eeb576977b26c5c1cd0364f15ba9d198bb16101e7563ab06/"
        f"onnxruntime_ep_webgpu-{WEBGPU_VERSION}-py3-none-win_amd64.whl",
        "7db646669d1a2390551da675115ea125e68cb3d4d2c3a2d6e463372bf8d6ea87",
        ["onnxruntime_providers_webgpu.dll", "dxcompiler.dll", "dxil.dll"],
    ),
    "windows-arm64": (
        f"onnxruntime-win-arm64-{ORT_VERSION}.zip",
        "e53db8a50b23ae35be901cc93428baf997dc8d420333b097b2eae53d3ea9f2d3",
        "lib/onnxruntime.dll",
        "cd/35/e1b1b12e596354d05af43cea09cd9fbd8a24b6df3853c527307b5cfb8d4b/"
        f"onnxruntime_ep_webgpu-{WEBGPU_VERSION}-py3-none-win_arm64.whl",
        "a9586f6744370776059edd794c11bca2f563258a5bd2707864aefff31d3af39f",
        ["onnxruntime_providers_webgpu.dll", "dxcompiler.dll", "dxil.dll"],
    ),
}
# 2026-09-16, the plugin's release date.
TIMESTAMP = (2026, 9, 16, 0, 0, 0)
MTIME = 1789516800


def fetch(url, sha256, downloads):
    cached = downloads / url.rsplit("/", 1)[1] if downloads else None
    if cached and cached.is_file():
        data = cached.read_bytes()
    else:
        with urllib.request.urlopen(url, timeout=300) as response:
            data = response.read()
    actual = hashlib.sha256(data).hexdigest()
    if actual != sha256:
        raise ValueError(f"{url}: expected SHA-256 {sha256}, found {actual}")
    if cached and not cached.is_file():
        cached.write_bytes(data)
    return data


class CoreArchive:
    """Members of Microsoft's archive, by path below its top folder."""

    def __init__(self, name, data):
        self.members = {}
        if name.endswith(".zip"):
            with zipfile.ZipFile(io.BytesIO(data)) as archive:
                for info in archive.infolist():
                    if not info.is_dir():
                        self.members[info.filename.split("/", 1)[1]] = archive.read(info)
        else:
            with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
                for info in archive.getmembers():
                    if info.isfile():
                        path = info.name.removeprefix("./").split("/", 1)[1]
                        self.members[path] = archive.extractfile(info).read()

    def read(self, path):
        return self.members[path]


def plugin_files(data, names):
    with zipfile.ZipFile(io.BytesIO(data)) as wheel:
        files = {name: wheel.read(f"onnxruntime_ep_webgpu/{name}") for name in names}
        licenses = f"onnxruntime_ep_webgpu-{WEBGPU_VERSION}.dist-info/licenses"
        files["LICENSE"] = wheel.read(f"{licenses}/LICENSE")
        files["ThirdPartyNotices.txt"] = wheel.read(f"{licenses}/ThirdPartyNotices.txt")
    return files


def write_tgz(path, root, members):
    buffer = io.BytesIO()
    with gzip.GzipFile(filename="", mode="wb", fileobj=buffer, compresslevel=9, mtime=0) as gz:
        with tarfile.open(fileobj=gz, mode="w", format=tarfile.USTAR_FORMAT) as archive:
            for name, data in members:
                info = tarfile.TarInfo(f"{root}/{name}")
                info.size = len(data)
                info.mtime = MTIME
                info.mode = 0o755 if name.startswith("lib/") else 0o644
                info.uname = info.gname = ""
                archive.addfile(info, io.BytesIO(data))
    path.write_bytes(buffer.getvalue())


def write_zip(path, root, members):
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for name, data in members:
            info = zipfile.ZipInfo(f"{root}/{name}", TIMESTAMP)
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o644 << 16
            archive.writestr(info, data)


def main():
    if len(sys.argv) not in (2, 3):
        sys.exit(__doc__)
    output = Path(sys.argv[1])
    downloads = Path(sys.argv[2]) if len(sys.argv) == 3 else None
    if downloads:
        downloads.mkdir(parents=True, exist_ok=True)
    for platform, (core_name, core_sha256, library, wheel, wheel_sha256, plugin) in PLATFORMS.items():
        core = CoreArchive(core_name, fetch(f"{ORT_RELEASE}/{core_name}", core_sha256, downloads))
        webgpu = plugin_files(fetch(f"{WEBGPU_WHEELS}/{wheel}", wheel_sha256, downloads), plugin)
        members = [(library, core.read(library))]
        members += [(f"lib/{name}", webgpu[name]) for name in plugin]
        members += [
            ("LICENSE", core.read("LICENSE")),
            ("Privacy.md", core.read("Privacy.md")),
            ("ThirdPartyNotices.txt", core.read("ThirdPartyNotices.txt")),
            ("webgpu-LICENSE", webgpu["LICENSE"]),
            ("webgpu-ThirdPartyNotices.txt", webgpu["ThirdPartyNotices.txt"]),
        ]
        members.sort()
        stem, extension = core_name.rsplit(".", 1)
        root = f"{stem}-webgpu-{WEBGPU_VERSION}"
        archive_path = output / platform / f"{root}.{extension}"
        archive_path.parent.mkdir(parents=True, exist_ok=True)
        (write_zip if extension == "zip" else write_tgz)(archive_path, root, members)
        digest = hashlib.sha256(archive_path.read_bytes()).hexdigest()
        print(f"{archive_path} bytes={archive_path.stat().st_size} sha256={digest}")


if __name__ == "__main__":
    main()
