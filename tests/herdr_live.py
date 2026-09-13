#!/usr/bin/env python3
"""Verify plugin actions against an installed Herdr in an isolated test session."""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import time


ROOT = Path(__file__).resolve().parents[1]


def verify(base, herdr):
    env = {key: value for key, value in os.environ.items()
           if not key.startswith("HERDR_") and key != "NO_COLOR"}
    for key, folder in [("XDG_CONFIG_HOME", "c"), ("XDG_STATE_HOME", "s"),
                        ("XDG_DATA_HOME", "d"), ("XDG_CACHE_HOME", "cache")]:
        env[key] = str(base / folder)
    cli = [herdr, "--session", "sidebar-test"]

    # A known working-tree change makes the width check independent of whether
    # the plugin checkout itself is clean (as it normally is in CI).
    repo = base / "repo"
    repo.mkdir()
    for args in [("init",), ("config", "user.name", "Graph test"),
                 ("config", "user.email", "graph-test@example.invalid")]:
        subprocess.run(["git", "-C", str(repo), *args], check=True, capture_output=True)
    (repo / "example.txt").write_text("initial\n")
    subprocess.run(["git", "-C", str(repo), "add", "example.txt"], check=True)
    subprocess.run(["git", "-C", str(repo), "-c", "commit.gpgsign=false", "commit",
                    "-m", "Initial history"], check=True, capture_output=True)
    (repo / "example.txt").write_text("uncommitted\n")

    def run(*args, check=True):
        result = subprocess.run([*cli, *args], cwd=ROOT, env=env, text=True,
                                capture_output=True, timeout=15)
        if check and result.returncode:
            raise AssertionError((args, result.stdout, result.stderr))
        return result

    def data(*args):
        return json.loads(run(*args).stdout)["result"]

    def invoke(action):
        result = data("plugin", "action", "invoke", "herdr.git-graph." + action)
        log_id = result["log"]["log_id"]
        for _ in range(100):
            logs = data("plugin", "log", "list", "--plugin", "herdr.git-graph")["logs"]
            log = next(item for item in logs if item["log_id"] == log_id)
            if log["status"] != "running":
                assert log["status"] == "succeeded", log
                return json.loads(log["stdout"])["result"]["plugin_pane"]["pane"]
            time.sleep(0.1)
        raise AssertionError("Plugin action did not finish")

    with (base / "server.log").open("w") as log_file:
        server = subprocess.Popen([*cli, "server"], cwd=ROOT, env=env,
                                  stdout=log_file, stderr=log_file)
        try:
            for _ in range(50):
                if "status: running" in run("status", "server", check=False).stdout:
                    break
                assert server.poll() is None, (base / "server.log").read_text()
                time.sleep(0.1)
            else:
                raise AssertionError("Test server did not start")

            run("plugin", "link", str(ROOT))
            created = data("workspace", "create", "--cwd", str(repo),
                           "--label", "Sidebar test", "--focus")
            source = created["root_pane"]["pane_id"]
            source_tab = created["tab"]["tab_id"]
            workspace = created["workspace"]["workspace_id"]

            # Reproduce the v0.2.0 failure against Herdr's actual validation.
            invalid = run("plugin", "pane", "open", "--plugin", "herdr.git-graph",
                          "--entrypoint", "sidebar", "--placement", "split",
                          "--workspace", workspace, "--target-pane", source,
                          "--direction", "right", "--cwd", str(ROOT), "--no-focus",
                          check=False)
            assert invalid.returncode != 0
            assert "use target_pane_id" in invalid.stderr, invalid.stderr

            # Invoke as a background action, without inherited caller pane IDs.
            sidebar = invoke("sidebar")
            assert sidebar["tab_id"] == source_tab, sidebar
            assert sidebar["pane_id"] != source, sidebar
            assert not sidebar["focused"], sidebar
            layout = data("pane", "layout", "--pane", sidebar["pane_id"])["layout"]
            graph_pane = next(p for p in layout["panes"] if p["pane_id"] == sidebar["pane_id"])
            assert graph_pane["rect"]["width"] == 40, layout
            assert layout["focused_pane_id"] == source, layout
            for _ in range(50):
                screen = run("pane", "read", sidebar["pane_id"],
                             "--source", "visible", "--raw").stdout
                if "GIT GRAPH" in screen:
                    break
                time.sleep(0.1)
            assert "GIT GRAPH" in screen, screen
            assert "COMMIT INSPECTOR" not in screen, screen
            assert "Enlarge" not in screen, screen
            assert "Uncommitted changes" in screen, screen
            assert "? help" in screen and "q quit" in screen, screen

            # Auto starts with symbolic ANSI colors, then replaces lane/accent
            # colors with actual RGB replies from Herdr's pane terminal.
            for _ in range(50):
                ansi = run("pane", "read", sidebar["pane_id"], "--source", "visible",
                           "--ansi", "--raw").stdout
                if re.search(r"38[;:]2[;:]", ansi):
                    break
                time.sleep(0.1)
            assert re.search(r"38[;:]2[;:]", ansi), repr(ansi)
            assert "48;2;12;17;24" not in ansi, "Classic background leaked into auto theme"

            # A new graph beside an already split source must resize only its
            # own divider, leaving the first graph's width and source focus intact.
            nested = invoke("sidebar")
            layout = data("pane", "layout", "--pane", nested["pane_id"])["layout"]
            widths = {p["pane_id"]: p["rect"]["width"] for p in layout["panes"]}
            assert widths[sidebar["pane_id"]] == 40, layout
            assert widths[nested["pane_id"]] == 40, layout
            assert layout["focused_pane_id"] == source, layout

            full = invoke("open")
            assert full["workspace_id"] == workspace, full
            assert full["tab_id"] != source_tab, full
            assert full["focused"], full
            print("PASS: actual Herdr sidebar action, 40-column/nested splits, preserved focus, visible hints, OSC theme replies, graph output, full-view action")
        finally:
            # All commands use the test session and isolated XDG paths.
            run("server", "stop", check=False)
            try:
                server.wait(timeout=5)
            except subprocess.TimeoutExpired:
                server.terminate()
                server.wait(timeout=5)


if __name__ == "__main__":
    herdr = shutil.which("herdr")
    if not herdr:
        raise SystemExit("Install Herdr before running this optional integration test.")
    with tempfile.TemporaryDirectory(prefix="hgg-live-", dir="/tmp") as temporary:
        verify(Path(temporary), herdr)
