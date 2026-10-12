#!/usr/bin/env python3
"""Build the Windows Automatic ONNX Runtime archives for CalibRaw Artifacts.

Usage: python3 packaging/onnxruntime/package_windows_directml.py OUTPUT_DIR

Combines Microsoft's DirectML ONNX Runtime NuGet package with the
redistributable DirectML.dll. ONNX Runtime's delay-load hook loads DirectML.dll
from the directory containing onnxruntime.dll, so both stay in lib/. Output is
byte-reproducible; the printed size and SHA-256 go into onnx_runtime_artifact.rs.
"""

import hashlib
import io
import sys
import urllib.request
import zipfile
from pathlib import Path

ORT_VERSION = "1.24.4"
ORT_PACKAGE = (
    f"https://www.nuget.org/api/v2/package/Microsoft.ML.OnnxRuntime.DirectML/{ORT_VERSION}",
    "57e9f11b73437bef7a309496135d4c1f96b1a8e9ddba60013fa27bfc1d788681",
)
DIRECTML_PACKAGE = (
    "https://www.nuget.org/api/v2/package/Microsoft.AI.DirectML/1.15.4",
    "4e7cb7ddce8cf837a7a75dc029209b520ca0101470fcdf275c1f49736a3615b9",
)
# CalibRaw platform -> (archive arch, ONNX Runtime RID, DirectML RID)
PLATFORMS = {
    "windows-x86_64": ("x64", "win-x64", "x64-win"),
    "windows-arm64": ("arm64", "win-arm64", "arm64-win"),
}
TIMESTAMP = (2026, 3, 17, 0, 0, 0)


def fetch(url, sha256):
    with urllib.request.urlopen(url, timeout=300) as response:
        data = response.read()
    actual = hashlib.sha256(data).hexdigest()
    if actual != sha256:
        raise ValueError(f"{url}: expected SHA-256 {sha256}, found {actual}")
    return zipfile.ZipFile(io.BytesIO(data))


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    output = Path(sys.argv[1])
    ort, directml = fetch(*ORT_PACKAGE), fetch(*DIRECTML_PACKAGE)
    for platform, (arch, ort_rid, directml_rid) in PLATFORMS.items():
        root = f"onnxruntime-win-{arch}-directml-{ORT_VERSION}"
        native = f"runtimes/{ort_rid}/native"
        members = [
            ("lib/onnxruntime.dll", ort, f"{native}/onnxruntime.dll"),
            ("lib/onnxruntime_providers_shared.dll", ort, f"{native}/onnxruntime_providers_shared.dll"),
            ("lib/DirectML.dll", directml, f"bin/{directml_rid}/DirectML.dll"),
            ("LICENSE", ort, "LICENSE"),
            ("ThirdPartyNotices.txt", ort, "ThirdPartyNotices.txt"),
            ("DirectML-LICENSE.txt", directml, "LICENSE.txt"),
            ("DirectML-ThirdPartyNotices.txt", directml, "ThirdPartyNotices.txt"),
        ]
        archive_path = output / platform / f"{root}.zip"
        archive_path.parent.mkdir(parents=True, exist_ok=True)
        with zipfile.ZipFile(archive_path, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
            for name, package, source in members:
                info = zipfile.ZipInfo(f"{root}/{name}", TIMESTAMP)
                info.compress_type = zipfile.ZIP_DEFLATED
                info.external_attr = 0o644 << 16
                archive.writestr(info, package.read(source))
        digest = hashlib.sha256(archive_path.read_bytes()).hexdigest()
        print(f"{archive_path} bytes={archive_path.stat().st_size} sha256={digest}")


if __name__ == "__main__":
    main()
