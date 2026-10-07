#!/usr/bin/env python3
"""Exercise the built Flatpak on its real runtime using a synthetic RAW."""

import hashlib
import struct
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def synthetic_dng(destination: Path) -> None:
    """Write a small uncompressed Bayer DNG without camera or third-party data."""
    width = height = 64
    tags = []

    def tag(number: int, kind: int, values) -> None:
        if kind == 2:
            data = values.encode() + b"\0"
            count = len(data)
        elif kind == 1:
            data = bytes(values)
            count = len(values)
        elif kind in (3, 4):
            data = struct.pack("<" + ("H" if kind == 3 else "I") * len(values), *values)
            count = len(values)
        else:
            data = b"".join(struct.pack("<ii" if kind == 10 else "<II", *value) for value in values)
            count = len(values)
        tags.append((number, kind, count, data))

    for number, kind, values in [
        (256, 4, [width]), (257, 4, [height]), (258, 3, [16]),
        (259, 3, [1]), (262, 3, [32803]), (271, 2, "CalibRaw"),
        (272, 2, "Synthetic Flatpak test"), (273, 4, [0]), (274, 3, [1]),
        (277, 3, [1]), (278, 4, [height]), (279, 4, [width * height * 2]),
        (284, 3, [1]), (339, 3, [1]), (33421, 3, [2, 2]),
        (33422, 1, [0, 1, 1, 2]), (50706, 1, [1, 4, 0, 0]),
        (50707, 1, [1, 4, 0, 0]), (50708, 2, "CalibRaw synthetic RAW"),
        (50710, 1, [0, 1, 2]), (50711, 3, [1]), (50714, 3, [64]),
        (50717, 4, [16383]), (50721, 10, [(int(i % 4 == 0), 1) for i in range(9)]),
        (50728, 5, [(1, 2), (1, 1), (1, 2)]), (50778, 3, [21]),
    ]:
        tag(number, kind, values)
    tags.sort()
    offset = 8 + 2 + 12 * len(tags) + 4
    entries = bytearray()
    extra = bytearray()
    for number, kind, count, data in tags:
        value = data.ljust(4, b"\0") if len(data) <= 4 else struct.pack("<I", offset + len(extra))
        entries.extend(struct.pack("<HHI", number, kind, count) + value)
        if len(data) > 4:
            extra.extend(data)
            if len(extra) % 2:
                extra.append(0)
    pixel_offset = offset + len(extra)
    strip_entry = next(i for i, value in enumerate(tags) if value[0] == 273)
    struct.pack_into("<I", entries, strip_entry * 12 + 8, pixel_offset)
    pixels = [1024 + (x + y) * 80 for y in range(height) for x in range(width)]
    destination.write_bytes(
        b"II" + struct.pack("<HIH", 42, 8, len(tags)) + entries + b"\0" * 4
        + extra + struct.pack("<" + "H" * len(pixels), *pixels)
    )


def main() -> None:
    build = ROOT / ".flatpak/build"
    library = build / "files/lib/libonnxruntime.so"
    if not library.is_file():
        raise SystemExit("Build the Flatpak first: scripts/flatpak.sh build")
    work = ROOT / ".flatpak/smoke"
    work.mkdir(parents=True, exist_ok=True)
    raw = work / "synthetic.dng"
    output = work / "synthetic.png"
    synthetic_dng(raw)
    output.unlink(missing_ok=True)
    base = [
        "flatpak", "run", "--user", "--branch=local",
        "--unshare=network", f"--filesystem={work}",
    ]
    runtime_hash = hashlib.sha256(library.read_bytes()).hexdigest()
    subprocess.run(base + [
        "--command=calibraw", "io.github.Duecki1.CalibRaw", "--calibraw-onnx-runtime-probe",
        "/app/lib/libonnxruntime.so", runtime_hash,
    ], check=True)
    subprocess.run(base + [
        "--command=calibraw-develop-export", "io.github.Duecki1.CalibRaw",
        "--input", str(raw), "--output", str(output),
    ], check=True)
    png = output.read_bytes()
    if png[:8] != b"\x89PNG\r\n\x1a\n" or struct.unpack(">II", png[16:24]) != (64, 64):
        raise SystemExit("Synthetic RAW export failed PNG format or dimensions check")
    print(f"Passed: offline ONNX Runtime load and synthetic RAW GPU export ({len(png)} bytes).")
    print(f"Inspect the output at {output}; interactive folder, sidecar, AI model and Trash checks remain manual.")


if __name__ == "__main__":
    main()
