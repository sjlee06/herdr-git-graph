#!/usr/bin/env python3
"""Refresh built-in demo SVGs and 2x PNGs, preserving the Herdr workspace frame.

Requires Cargo and resvg on PATH. Run from any working directory.
"""
from pathlib import Path
import shutil
import subprocess
import tempfile
import xml.etree.ElementTree as ET


ROOT = Path(__file__).resolve().parents[1]
SVG = "http://www.w3.org/2000/svg"
ET.register_namespace("", SVG)


def compose_workspace(path, content):
    # The captured Herdr frame has a 40-column split, including its borders.
    # Refresh only its 38x39 content cells; leave the editor and terminal intact.
    frame = ET.parse(path).getroot()
    for parent in list(frame.iter()):
        for child in list(parent):
            if child.get("id") == "preview-graph-pane":
                parent.remove(child)
                continue
            if "x" not in child.attrib or "y" not in child.attrib:
                continue
            x, y = float(child.get("x")), float(child.get("y"))
            if 1210 <= x < 1590 and 40 <= y < 820:
                parent.remove(child)

    graph = ET.parse(content).getroot()
    graph.set("id", "preview-graph-pane")
    graph.set("x", "1210")
    graph.set("y", "40")
    graph.set("width", "380")
    graph.set("height", "780")
    # Match the captured terminal's 10x20 cells to the snapshot's 9x20 cells.
    graph.set("preserveAspectRatio", "none")
    frame.append(graph)
    ET.ElementTree(frame).write(path, encoding="unicode")
    print(f"Workspace preview: {path}")


def main():
    resvg = shutil.which("resvg")
    if resvg is None:
        raise SystemExit("resvg is required on PATH to render the 2x PNG previews")

    docs = ROOT / "docs"
    with tempfile.TemporaryDirectory(prefix="herdr-graph-preview-") as directory:
        output = Path(directory)
        subprocess.run(
            ["cargo", "run", "--quiet", "--locked", "--example", "previews", "--", str(output)],
            cwd=ROOT, check=True,
        )
        for source in sorted(output.glob("preview*.svg")):
            shutil.copyfile(source, docs / source.name)
            print(f"Preview: {docs / source.name}")
        compose_workspace(docs / "preview-sidebar.svg", output / "workspace-content.svg")

    # Include the preserved workspace composite and the text-renderer preview.
    for source in sorted(docs.glob("preview*.svg")):
        target = source.with_suffix(".png")
        subprocess.run([resvg, "--zoom", "2", str(source), str(target)], check=True)
        print(f"Raster preview: {target}")


if __name__ == "__main__":
    main()
