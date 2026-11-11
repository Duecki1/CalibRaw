#!/usr/bin/env python3
"""Build the OpenVINO execution provider archives for Intel CPUs.

Usage: python3 packaging/onnxruntime/package_openvino.py OUTPUT_DIR [DOWNLOAD_DIR]

Each archive holds Intel's OpenVINO plugin execution provider for ONNX Runtime
(`onnxruntime-ep-openvino` on PyPI) and the part of the OpenVINO runtime it
bundles that runs on the CPU, unmodified, in lib/: OpenVINO's core, its ONNX
and IR frontends, its CPU plugin, oneTBB and hwloc (linked into tbbbind on
Windows). The wheel's
GPU and NPU plugins, the AUTO, HETERO and batching plugins, the GPU kernel
cache and tbbmalloc_proxy are left out. CalibRaw downloads the archive on an
Intel CPU in Automatic runtime mode while "Use OpenVINO on Intel CPUs" is on,
and registers the plugin with the automatic ONNX Runtime 1.30.0 (the plugin
needs 1.23 or newer).

On Windows the plugin's embedded manifest depends on the private assembly
`openvino_runtime`, described by lib/openvino_runtime.manifest. The archive
keeps that manifest with only the files it ships (`cpu_manifest`), and the
script checks that every library finds what it imports (`check_imports`).

The wheel has no license file of its own: its metadata and sources declare
the MIT license, copyright Intel Corporation, so LICENSE carries that text.
OpenVINO and oneTBB are Apache-2.0 and hwloc BSD-3-Clause; their license
files are fetched from their release tags.

Downloads are SHA-256 pinned and kept in DOWNLOAD_DIR when it is given. The
output is byte-reproducible with the same Python and zlib. Upload each archive
unchanged to the CalibRaw-Artifacts repository at onnxruntime/openvino/<platform>/,
the path it is written to under OUTPUT_DIR, and copy the printed size and
SHA-256 into crates/calibraw-ai/src/runtime/openvino.rs.
"""

import hashlib
import io
import re
import sys
import zipfile
from pathlib import Path

from package_webgpu import WINDOWS_SYSTEM_DLLS, fetch, pe_imports, write_tgz, write_zip

PLUGIN_VERSION = "1.8.1"
OPENVINO_VERSION = "2026.4.1"
WHEELS = "https://files.pythonhosted.org/packages"
PACKAGE = "onnxruntime_ep_openvino"

# CalibRaw platform -> wheel URL path, its SHA-256, archive extension, and
# the files taken from the wheel's package folder, the plugin first.
PLATFORMS = {
    "linux-x86_64": (
        "88/01/4e72a8a65b584f01d6e59eb453bae397b82b2e51df1b5c94efa05e17800b/"
        f"{PACKAGE}-{PLUGIN_VERSION}-py3-none-manylinux_2_28_x86_64.whl",
        "3832643158c8c187fcab6db661cde2a8c06a2b3559d95d0f2411ac0e51c5e924",
        "tgz",
        [
            # The wheel carries this library three times, under its soname
            # and version too; nothing links to it, so its plain name serves.
            "libonnxruntime_providers_openvino_plugin.so",
            "libopenvino.so.2641",
            "libopenvino_onnx_frontend.so.2641",
            "libopenvino_ir_frontend.so.2641",
            "libopenvino_intel_cpu_plugin.so",
            "libtbb.so.12",
            "libtbbmalloc.so.2",
            # oneTBB loads these to place threads on hybrid and NUMA CPUs.
            "libtbbbind_2_5.so.3",
            "libhwloc.so.15",
        ],
    ),
    "windows-x86_64": (
        "75/98/224ead07344390258ae0515ed285ba2951e9c1405f4507443a03bb7dd5c7/"
        f"{PACKAGE}-{PLUGIN_VERSION}-py3-none-win_amd64.whl",
        "1acd878be173b02bb8b953460f2f59b121e5f6e47ebc6aad490c0590cf6718fb",
        "zip",
        [
            "onnxruntime_providers_openvino_plugin.dll",
            "openvino.dll",
            "openvino_onnx_frontend.dll",
            "openvino_ir_frontend.dll",
            "openvino_intel_cpu_plugin.dll",
            "tbb12.dll",
            "tbbmalloc.dll",
            "tbbbind_2_5.dll",
        ],
    ),
}
# License files: archive name -> URL and SHA-256. OpenVINO and oneTBB ship
# the same Apache-2.0 text.
LICENSES = {
    "openvino-LICENSE": (
        f"https://raw.githubusercontent.com/openvinotoolkit/openvino/{OPENVINO_VERSION}/LICENSE",
        "c71d239df91726fc519c6eb72d318ec65820627232b2f796219e87dcf35d0ab4",
    ),
    "oneTBB-LICENSE.txt": (
        "https://raw.githubusercontent.com/uxlfoundation/oneTBB/v2021.13.1/LICENSE.txt",
        "c71d239df91726fc519c6eb72d318ec65820627232b2f796219e87dcf35d0ab4",
    ),
    "hwloc-COPYING": (
        "https://raw.githubusercontent.com/open-mpi/hwloc/hwloc-2.9.3/COPYING",
        "d79a936a42f3c6cb7c8375a023d43f4435f4664d3a5a2ea6b4623cff83c7fc06",
    ),
}
# The license the wheel declares (`License: MIT` in its metadata, SPDX MIT
# with this copyright in its sources), which it does not include as a file.
PLUGIN_LICENSE = """\
MIT License

Copyright (C) 2026 Intel Corporation

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
"""
MANIFEST = "openvino_runtime.manifest"
# Windows libraries the plugin's files import besides WINDOWS_SYSTEM_DLLS and
# the api-ms-win-* API sets; they link the C++ runtime statically.
WINDOWS_EXTRA_SYSTEM_DLLS = {"dbghelp.dll"}


def wheel_files(data, names):
    """`names` from the wheel's package folder, and its license notices."""
    with zipfile.ZipFile(io.BytesIO(data)) as wheel:
        files = {name: wheel.read(f"{PACKAGE}/{name}") for name in names}
        files["third-party-programs.txt"] = wheel.read(f"{PACKAGE}/third-party-programs.txt")
        if f"{PACKAGE}/{MANIFEST}" in wheel.namelist():
            files[MANIFEST] = wheel.read(f"{PACKAGE}/{MANIFEST}")
    return files


def cpu_manifest(original, shipped):
    """The wheel's assembly manifest with the `<file>` entries of the
    libraries this archive leaves out removed. Fails unless it lists every
    shipped library but the plugin, which is not part of the assembly."""
    text = original.decode("utf-8")
    listed = set(re.findall(r'<file name="([^"]+)"\s*/>', text))
    expected = set(shipped[1:])
    if not expected <= listed:
        raise ValueError(f"{MANIFEST} does not list {', '.join(sorted(expected - listed))}")
    for name in listed - expected:
        text, count = re.subn(rf'\s*<file name="{re.escape(name)}"\s*/>', "", text)
        if count != 1:
            raise ValueError(f"{MANIFEST} lists {name} {count} times")
    notice = "<!-- CalibRaw removed the files of the GPU, NPU, AUTO, HETERO and batching plugins it does not ship. -->"
    return text.replace("<assembly ", f"{notice}\n<assembly ", 1).encode("utf-8")


def check_imports(platform, libraries):
    """Fails unless every library's imports are another shipped library or
    part of Windows. ONNX Runtime loads the plugin with
    `LOAD_WITH_ALTERED_SEARCH_PATH`, so its imports are found beside it."""
    system = WINDOWS_SYSTEM_DLLS | WINDOWS_EXTRA_SYSTEM_DLLS
    shipped = {name.lower() for name in libraries}
    for name, data in sorted(libraries.items()):
        missing = sorted(
            library
            for library in pe_imports(data)
            if not library.startswith(("api-ms-win-", "ext-ms-win-"))
            and library not in system | shipped
        )
        if missing:
            raise ValueError(f"{platform}: {name} imports {', '.join(missing)}, which the archive lacks")


def main():
    if len(sys.argv) not in (2, 3):
        sys.exit(__doc__)
    output = Path(sys.argv[1])
    downloads = Path(sys.argv[2]) if len(sys.argv) == 3 else None
    if downloads:
        downloads.mkdir(parents=True, exist_ok=True)
    licenses = {name: fetch(url, sha256, downloads) for name, (url, sha256) in LICENSES.items()}
    for platform, (wheel, wheel_sha256, extension, names) in PLATFORMS.items():
        files = wheel_files(fetch(f"{WHEELS}/{wheel}", wheel_sha256, downloads), names)
        members = [(f"lib/{name}", files[name]) for name in names]
        if extension == "zip":
            check_imports(platform, {name: files[name] for name in names})
            members.append((f"lib/{MANIFEST}", cpu_manifest(files[MANIFEST], names)))
        members += [
            ("LICENSE", PLUGIN_LICENSE.encode("utf-8")),
            ("third-party-programs.txt", files["third-party-programs.txt"]),
        ]
        # Windows' tbbbind_2_5.dll links hwloc in statically.
        members += list(licenses.items())
        members.sort()
        root = f"openvino-ep-{PLUGIN_VERSION}-cpu-{platform}"
        archive_path = output / "openvino" / platform / f"{root}.{extension}"
        archive_path.parent.mkdir(parents=True, exist_ok=True)
        (write_zip if extension == "zip" else write_tgz)(archive_path, root, members)
        digest = hashlib.sha256(archive_path.read_bytes()).hexdigest()
        print(f"{archive_path} bytes={archive_path.stat().st_size} sha256={digest}")


if __name__ == "__main__":
    main()
