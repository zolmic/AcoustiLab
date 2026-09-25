#!/usr/bin/env python3
"""Exports the printable parts of the open reference cup as binary STL.

    python3 tools/refcup/dims_to_scad.py     # first, if dimensions.json changed
    python3 tools/refcup/export_stl.py       # needs OpenSCAD on the PATH

Writes validation/reference_cup/geometry/stl/<part>.stl, each in its print
orientation, and stl/SHA256SUMS, which records the OpenSCAD version and the
SHA-256 of the SCAD sources the parts came from. Standard library only.
"""
import hashlib
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
GEOM = ROOT / "validation/reference_cup/geometry"
PARTS = [
    "baffle",
    "retainer",
    "pad_ring",
    "rear_shell",
    "plug_rear_sealed",
    "plug_rear_hole",
    "plug_rear_mesh",
    "plug_front_sealed",
    "plug_front_mesh",
]


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    check = subprocess.run(
        [sys.executable, str(ROOT / "tools/refcup/dims_to_scad.py"), "--check"],
        capture_output=True, text=True)
    if check.returncode:
        sys.exit(check.stderr.strip())
    version = subprocess.run(["openscad", "--version"], capture_output=True, text=True)
    version = (version.stdout + version.stderr).strip()
    out = GEOM / "stl"
    out.mkdir(exist_ok=True)
    lines = [
        f"# {version}",
        f"# reference_cup.scad {sha256(GEOM / 'reference_cup.scad')}",
        f"# dimensions.scad {sha256(GEOM / 'dimensions.scad')}",
    ]
    for part in PARTS:
        stl = out / f"{part}.stl"
        subprocess.run(
            ["openscad", "--export-format", "binstl", "-o", str(stl),
             "-D", f'part="{part}"', str(GEOM / "reference_cup.scad")],
            check=True, capture_output=True)
        lines.append(f"{sha256(stl)}  {part}.stl")
        print(f"wrote {stl.relative_to(ROOT)}")
    (out / "SHA256SUMS").write_text("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()
