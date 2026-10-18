#!/usr/bin/env python3
"""Refresh demo SVGs from the built viewer, preserving the Herdr workspace frame."""
from pathlib import Path
import subprocess
import tempfile
import xml.etree.ElementTree as ET


ROOT = Path(__file__).resolve().parents[1]
SVG = "http://www.w3.org/2000/svg"
ET.register_namespace("", SVG)


def snapshot(path, width, height, renderer="curves", sidebar=False):
    command = [str(ROOT / "bin/herdr-git-graph"), "--demo", "--theme", "classic",
               "--renderer", renderer, "--width", str(width), "--height", str(height),
               "--snapshot", str(path)]
    if sidebar:
        command.append("--sidebar")
    subprocess.run(command, cwd=ROOT, check=True)


def main():
    snapshot(ROOT / "docs/preview.svg", 140, 56)
    snapshot(ROOT / "docs/preview-text.svg", 140, 56, renderer="text")
    snapshot(ROOT / "docs/preview-sidebar-dark.svg", 40, 26, sidebar=True)

    # The captured Herdr frame has a 40-column split, including its borders.
    # Refresh only its 38x39 content cells; leave the editor and terminal intact.
    path = ROOT / "docs/preview-sidebar.svg"
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

    with tempfile.TemporaryDirectory(prefix="herdr-graph-preview-") as directory:
        sidebar = Path(directory) / "sidebar.svg"
        snapshot(sidebar, 38, 39, sidebar=True)
        graph = ET.parse(sidebar).getroot()
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


if __name__ == "__main__":
    main()
