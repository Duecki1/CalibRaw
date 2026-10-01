#!/usr/bin/env python3
"""Bundle the license texts from Flatpak's offline Cargo source tree."""

from pathlib import Path
import tomllib


def main() -> None:
    root = Path.cwd()
    vendor = root / "cargo/vendor"
    if not vendor.is_dir():
        raise SystemExit("Missing offline Cargo sources: cargo/vendor")
    sections = ["# Rust dependency licenses\n\nGenerated from vendored Cargo.lock sources.\n"]
    for package in sorted(vendor.iterdir()):
        manifest = package / "Cargo.toml"
        if not manifest.is_file():
            continue
        metadata = tomllib.loads(manifest.read_text())["package"]
        sections.append(f"\n## {metadata['name']} {metadata['version']}\n")
        sections.append(f"\nLicense: {metadata.get('license', 'See included license file')}\n")
        licenses = set()
        for pattern in ("LICENSE*", "LICENCE*", "COPYING*", "NOTICE*", "UNLICENSE*"):
            licenses.update(path for path in package.glob(pattern) if path.is_file())
        if filename := metadata.get("license-file"):
            path = package / filename
            if not path.is_file():
                raise SystemExit(f"Missing declared license file: {path}")
            licenses.add(path)
        for path in sorted(licenses):
            text = path.read_text(encoding="utf-8", errors="replace").replace("\r\n", "\n")
            sections.append(f"\n### {path.name}\n\n```text\n{text.rstrip()}\n```\n")
    (root / "THIRD_PARTY_LICENSES.md").write_text("".join(sections), encoding="utf-8")


if __name__ == "__main__":
    main()
