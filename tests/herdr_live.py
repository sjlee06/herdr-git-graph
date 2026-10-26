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


def wait_for(description, probe, ready=bool, timeout=10):
    """Wait for an asynchronous server/plugin update, with useful diagnostics."""
    deadline = time.monotonic() + timeout
    observed = None
    while time.monotonic() < deadline:
        observed = probe()
        if ready(observed):
            return observed
        time.sleep(0.1)
    raise AssertionError(f"Timed out waiting for {description}: {observed!r}")


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
    # Enough rows to exercise selection and scroll preservation, not merely
    # process identity. Empty commits keep this fixture small and deterministic.
    for index in range(1, 37):
        subprocess.run(["git", "-C", str(repo), "-c", "commit.gpgsign=false",
                        "commit", "--allow-empty", "-m", f"History {index:02}"],
                       check=True, capture_output=True)
    (repo / "example.txt").write_text("uncommitted\n")

    def run(*args, check=True):
        result = subprocess.run([*cli, *args], cwd=ROOT, env=env, text=True,
                                capture_output=True, timeout=15)
        if check and result.returncode:
            raise AssertionError((args, result.stdout, result.stderr))
        return result

    def data(*args):
        return json.loads(run(*args).stdout)["result"]

    def logs():
        return data("plugin", "log", "list", "--plugin", "herdr.git-graph",
                    "--limit", "200")["logs"]

    def invoke_result(action):
        result = data("plugin", "action", "invoke", "herdr.git-graph." + action)
        log_id = result["log"]["log_id"]
        log = wait_for(
            f"{action} action completion",
            lambda: next((item for item in logs() if item["log_id"] == log_id), None),
            lambda item: item is not None and item["status"] != "running")
        assert log["status"] == "succeeded", log
        return json.loads(log["stdout"])["result"]

    def invoke(action):
        return invoke_result(action)["plugin_pane"]["pane"]

    def hooks_since(previous, minimum=1, expected_status="succeeded", event="tab.focused"):
        # For negative assertions (after closing), waiting for the actual hooks
        # is essential: checking absence immediately after focus would race them.
        hooks = wait_for(
            f"{event} hooks to finish",
            lambda: [item for item in logs() if item.get("event") == event
                     and item["log_id"] not in previous],
            lambda items: len(items) >= minimum
                          and all(item["status"] != "running" for item in items))
        for hook in hooks:
            assert hook["status"] == expected_status, hook
            assert "--sync-sidebar" in hook["command"], hook
            assert not (hook.get("stdout") or "").strip(), hook
            if expected_status == "failed":
                assert (hook.get("stderr") or "").strip(), hook
        return hooks

    def focus_tabs(*tabs, expected_status="succeeded"):
        previous = {item["log_id"] for item in logs()}
        # Deliberately do not wait between commands: callers can exercise rapid
        # focus sequences whose earlier event contexts are already stale.
        for tab in tabs:
            run("tab", "focus", tab)
        return hooks_since(previous, minimum=len(tabs), expected_status=expected_status)

    def panes(workspace):
        return data("pane", "list", "--workspace", workspace)["panes"]

    def saved_view(pane):
        directory = base / "s/herdr/plugins/herdr.git-graph/shared-sidebars"
        for path in directory.glob("*.json"):
            try:
                record = json.loads(path.read_text())
            except (FileNotFoundError, json.JSONDecodeError):
                continue
            if record.get("pane_id") == pane["pane_id"]:
                return record
        return {}

    def layout(pane):
        return data("pane", "layout", "--pane", pane)["layout"]

    def width(snapshot, pane):
        return next(item["rect"]["width"] for item in snapshot["panes"]
                    if item["pane_id"] == pane)

    def read_graph(pane):
        screen = run("pane", "read", pane, "--source", "visible", "--raw").stdout
        # Some Herdr versions include SGR even without --ansi. Strip it for
        # textual selection/viewport assertions; theme checks use raw ANSI.
        return re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", screen)

    def visible_history(screen):
        return tuple(re.findall(r"History \d{2}|Initial history|Uncommitted changes", screen))

    def graph_state(screen):
        # The sidebar footer describes the visible range, not the selection.
        # Read its persisted selection independently so an off-screen selection
        # cannot be mistaken for the last visible commit.
        count = re.search(r"\b\d+–\d+/(\d+)\s*·", screen)
        selected = saved_view(sidebar).get("view", {}).get("selected")
        subjects = visible_history(screen)
        if count and type(selected) is int and subjects:
            return ((str(selected + 1), count.group(1)), subjects)
        return None

    def shared_at(sidebar, workspace, tab, source, ordinary, remembered_width=None):
        def snapshot():
            members = panes(workspace)
            # Terminal identity also makes a failure diagnostic if Herdr ever
            # changes its same-workspace pane-ID preservation semantics.
            graphs = [pane for pane in members
                      if pane["terminal_id"] == sidebar["terminal_id"]]
            current = graphs[0] if len(graphs) == 1 else None
            current_layout = layout(current["pane_id"]) if current else None
            return members, current, current_layout

        members, current, current_layout = wait_for(
            f"shared sidebar in tab {tab} without stealing focus", snapshot,
            lambda state: state[1] is not None and state[1]["tab_id"] == tab
                          and state[2]["focused_pane_id"] == source
                          and (remembered_width is None or
                               abs(width(state[2], state[1]["pane_id"])
                                   - remembered_width) <= 1))
        assert current["pane_id"] == sidebar["pane_id"], (sidebar, current)
        assert current["workspace_id"] == workspace, current
        assert not current["focused"], current
        assert {pane["pane_id"] for pane in members} == ordinary | {sidebar["pane_id"]}, members
        assert {pane["pane_id"] for pane in current_layout["panes"]} == {
            source, sidebar["pane_id"]}, current_layout
        return current_layout

    def create_tab(workspace, label, cwd):
        result = data("tab", "create", "--workspace", workspace, "--cwd", str(cwd),
                      "--label", label, "--no-focus")
        return result["tab"]["tab_id"], result["root_pane"]["pane_id"]

    with (base / "server.log").open("w") as log_file:
        server = subprocess.Popen([*cli, "server"], cwd=ROOT, env=env,
                                  stdout=log_file, stderr=log_file)
        try:
            def server_status():
                assert server.poll() is None, (base / "server.log").read_text()
                return run("status", "server", check=False).stdout

            wait_for("test server startup", server_status,
                     lambda status: "status: running" in status, timeout=5)

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
            initial_layout = shared_at(sidebar, workspace, source_tab, source, {source}, 40)
            assert width(initial_layout, sidebar["pane_id"]) == 40, initial_layout
            screen = wait_for("initial graph output", lambda: read_graph(sidebar["pane_id"]),
                              lambda text: "GIT GRAPH" in text and "Uncommitted changes" in text)
            assert "COMMIT INSPECTOR" not in screen, screen
            assert "Enlarge" not in screen, screen
            assert "Uncommitted changes" in screen, screen
            assert "? help" in screen and "q quit" in screen, screen

            # Auto starts with symbolic ANSI colors, then replaces lane/accent
            # colors with actual RGB replies from Herdr's pane terminal.
            ansi = wait_for(
                "OSC theme replies",
                lambda: run("pane", "read", sidebar["pane_id"], "--source", "visible",
                            "--ansi", "--raw").stdout,
                lambda text: re.search(r"38[;:]2[;:]", text))
            assert "48;2;12;17;24" not in ansi, "Classic background leaked into auto theme"

            # Repeated actions reuse the live pane instead of making nested
            # graph splits, including when the sidebar itself has focus.
            repeated = invoke("sidebar")
            assert repeated["pane_id"] == sidebar["pane_id"], repeated
            assert repeated["terminal_id"] == sidebar["terminal_id"], repeated
            shared_at(sidebar, workspace, source_tab, source, {source}, 40)
            run("pane", "focus", "--pane", source, "--direction", "right")
            repeated = invoke("sidebar")
            assert repeated["pane_id"] == sidebar["pane_id"], repeated
            assert repeated["terminal_id"] == sidebar["terminal_id"], repeated
            assert layout(source)["focused_pane_id"] == sidebar["pane_id"]
            assert len(panes(workspace)) == 2, panes(workspace)

            # A shortcut-style toggle closes the focused sidebar, opts out of
            # tab hooks, and opens a new shared sidebar on the next invocation.
            previous = {item["log_id"] for item in logs()}
            invoke_result("toggle-sidebar")
            hooks_since(previous, event="pane.closed")
            assert {pane["pane_id"] for pane in panes(workspace)} == {source}
            assert not saved_view(sidebar)
            toggle_tab, _ = create_tab(workspace, "Toggle stays closed", base)
            focus_tabs(toggle_tab, source_tab)
            assert len(panes(workspace)) == 2, panes(workspace)
            previous = {item["log_id"] for item in logs()}
            run("tab", "close", toggle_tab)
            hooks_since(previous, event="tab.closed")
            assert {pane["pane_id"] for pane in panes(workspace)} == {source}
            replacement = invoke("toggle-sidebar")
            assert replacement["terminal_id"] != sidebar["terminal_id"], replacement
            sidebar = replacement
            shared_at(sidebar, workspace, source_tab, source, {source}, 40)
            wait_for("toggled-on graph output", lambda: read_graph(sidebar["pane_id"]),
                     lambda text: "GIT GRAPH" in text and "Uncommitted changes" in text)

            # Move well beyond the initial viewport and retain both the saved
            # selection index and visible history subjects across tab moves.
            run("pane", "send-keys", sidebar["pane_id"], "G")
            wait_for("selection at end of history", lambda: read_graph(sidebar["pane_id"]),
                     lambda text: graph_state(text) is not None
                                  and graph_state(text)[0] == ("38", "38"))
            run("pane", "send-keys", sidebar["pane_id"], "k", "k", "k")
            selected_screen = wait_for(
                "scrolled graph selection", lambda: read_graph(sidebar["pane_id"]),
                lambda text: graph_state(text) is not None
                             and graph_state(text)[0] == ("35", "38"))
            selected_state = graph_state(selected_screen)
            assert "Uncommitted changes" not in selected_screen, selected_screen
            run("pane", "focus", "--pane", sidebar["pane_id"], "--direction", "left")

            # Remember the user's actual width, allowing one column of rounding
            # when rebuilt in an equally sized destination tab.
            run("pane", "resize", "--pane", sidebar["pane_id"], "--direction", "left",
                "--amount", "0.05")
            remembered_width = width(layout(sidebar["pane_id"]), sidebar["pane_id"])
            assert remembered_width > 40, remembered_width
            selected_state = graph_state(wait_for(
                "graph redraw after manual resize", lambda: read_graph(sidebar["pane_id"]),
                lambda text: graph_state(text) == selected_state))
            tab2, source2 = create_tab(workspace, "Second tab", repo)
            # Auto-sync must not require a repository in the newly focused tab.
            tab3, source3 = create_tab(workspace, "Non-Git tab", base)
            ordinary = {source, source2, source3}

            def check_shared(tab, target):
                shared_at(sidebar, workspace, tab, target, ordinary, remembered_width)
                wait_for("preserved selection and scroll", lambda: read_graph(sidebar["pane_id"]),
                         lambda text: graph_state(text) == selected_state)

            focus_tabs(tab2)
            check_shared(tab2, source2)
            assert len(layout(source)["panes"]) == 1, layout(source)
            repeated = invoke("sidebar")
            assert repeated["pane_id"] == sidebar["pane_id"], repeated
            assert repeated["terminal_id"] == sidebar["terminal_id"], repeated
            check_shared(tab2, source2)
            focus_tabs(source_tab)
            check_shared(source_tab, source)
            assert len(layout(source2)["panes"]) == 1, layout(source2)
            focus_tabs(tab3)
            check_shared(tab3, source3)
            # Explicit reuse, not just the event hook, must also bypass Git
            # discovery in a non-repository destination shell.
            repeated = invoke("sidebar")
            assert repeated["pane_id"] == sidebar["pane_id"], repeated
            assert repeated["terminal_id"] == sidebar["terminal_id"], repeated
            check_shared(tab3, source3)
            focus_tabs(source_tab, tab2, tab3, tab2, source_tab, tab3, source_tab)
            check_shared(source_tab, source)
            assert len(layout(source2)["panes"]) == 1, layout(source2)
            assert len(layout(source3)["panes"]) == 1, layout(source3)

            # Sharing is workspace-local: another workspace starts without any
            # sidebar, gets its own graph on demand, and moves only that graph.
            other = data("workspace", "create", "--cwd", str(repo),
                         "--label", "Independent workspace", "--focus")
            other_workspace = other["workspace"]["workspace_id"]
            other_source = other["root_pane"]["pane_id"]
            other_tab = other["tab"]["tab_id"]
            assert {pane["pane_id"] for pane in panes(other_workspace)} == {other_source}
            other_sidebar = invoke("sidebar")
            assert other_sidebar["pane_id"] != sidebar["pane_id"], other_sidebar
            assert other_sidebar["terminal_id"] != sidebar["terminal_id"], other_sidebar
            shared_at(other_sidebar, other_workspace, other_tab, other_source, {other_source}, 40)
            other_tab2, other_source2 = create_tab(other_workspace, "Other second tab", repo)
            focus_tabs(other_tab2)
            shared_at(other_sidebar, other_workspace, other_tab2, other_source2,
                      {other_source, other_source2}, 40)
            focus_tabs(tab2)
            check_shared(tab2, source2)
            assert data("pane", "get", other_sidebar["pane_id"])["pane"]["tab_id"] == other_tab2
            assert {pane["pane_id"] for pane in panes(other_workspace)} == {
                other_source, other_source2, other_sidebar["pane_id"]}

            # Full view still creates and focuses a distinct graph tab. The
            # workspace's existing sidebar follows without taking its focus.
            previous = {item["log_id"] for item in logs()}
            full = invoke("open")
            assert full["workspace_id"] == workspace, full
            assert full["tab_id"] not in {source_tab, tab2, tab3}, full
            assert full["pane_id"] != sidebar["pane_id"], full
            assert full["terminal_id"] != sidebar["terminal_id"], full
            assert full["focused"], full
            ordinary.add(full["pane_id"])
            hooks_since(previous)
            shared_at(sidebar, workspace, full["tab_id"], full["pane_id"],
                      ordinary, remembered_width)
            wait_for("full-view graph", lambda: read_graph(full["pane_id"]),
                     lambda text: "HERDR" in text and "HISTORY" in text
                                  and ("WORKING TREE" in text or "COMMIT INSPECTOR" in text))
            focus_tabs(source_tab)
            check_shared(source_tab, source)

            # q and manual pane close both opt out. Subsequent completed focus
            # hooks must not resurrect the graph; only an explicit action can.
            run("pane", "focus", "--pane", source, "--direction", "right")
            run("pane", "send-keys", sidebar["pane_id"], "q")
            wait_for("q to close shared sidebar", lambda: panes(workspace),
                     lambda members: all(pane["terminal_id"] != sidebar["terminal_id"]
                                         for pane in members))
            focus_tabs(tab2, tab3, source_tab)
            assert {pane["pane_id"] for pane in panes(workspace)} == ordinary
            reopened = invoke("sidebar")
            assert reopened["terminal_id"] != sidebar["terminal_id"], reopened
            shared_at(reopened, workspace, source_tab, source, ordinary, 40)
            focus_tabs(tab2)
            shared_at(reopened, workspace, tab2, source2, ordinary, 40)
            run("pane", "close", reopened["pane_id"])
            focus_tabs(tab3, source_tab, tab2)
            assert {pane["pane_id"] for pane in panes(workspace)} == ordinary
            reopened_again = invoke("sidebar")
            assert reopened_again["terminal_id"] != reopened["terminal_id"], reopened_again
            shared_at(reopened_again, workspace, tab2, source2, ordinary, 40)

            # Herdr refuses to move from a zoomed source tab. The hook must
            # report failure (not successful sharing), leave user zoom alone,
            # and retain the original pane/process for explicit recovery.
            run("pane", "zoom", "--pane", source2, "--on")
            assert layout(source2)["zoomed"], layout(source2)
            focus_tabs(tab3, expected_status="failed")
            stranded = data("pane", "get", reopened_again["pane_id"])["pane"]
            assert stranded["tab_id"] == tab2, stranded
            assert stranded["terminal_id"] == reopened_again["terminal_id"], stranded
            assert layout(source2)["zoomed"], layout(source2)
            assert layout(source3)["focused_pane_id"] == source3, layout(source3)
            assert {pane["pane_id"] for pane in panes(workspace)} == \
                ordinary | {reopened_again["pane_id"]}
            # Address the known source explicitly: --current would unzoom the
            # wrong tab. Herdr focuses this source as part of its zoom command.
            run("pane", "zoom", "--pane", source2, "--off")
            assert not layout(source2)["zoomed"], layout(source2)
            assert data("workspace", "get", workspace)["workspace"]["active_tab_id"] == tab2
            focus_tabs(tab3)
            shared_at(reopened_again, workspace, tab3, source3, ordinary, 40)
            repeated = invoke("sidebar")
            assert repeated["pane_id"] == reopened_again["pane_id"], repeated
            assert repeated["terminal_id"] == reopened_again["terminal_id"], repeated
            shared_at(reopened_again, workspace, tab3, source3, ordinary, 40)
            assert data("pane", "get", other_sidebar["pane_id"])["pane"]["terminal_id"] == \
                other_sidebar["terminal_id"]
            # Closing the containing tab kills its process. The tab.closed hook
            # must create exactly one replacement on the surviving tab without
            # requiring another action or an extra focus event. A non-Git
            # survivor verifies that the saved repository is used.
            recovery = data("workspace", "create", "--cwd", str(base),
                            "--label", "Tab close recovery", "--focus")
            recovery_workspace = recovery["workspace"]["workspace_id"]
            recovery_tab = recovery["tab"]["tab_id"]
            recovery_source = recovery["root_pane"]["pane_id"]
            # First open needs Git context, even though the survivor is non-Git.
            second_tab, second_source = create_tab(recovery_workspace, "Close me", repo)
            focus_tabs(second_tab)
            original = invoke("sidebar")
            shared_at(original, recovery_workspace, second_tab, second_source,
                      {recovery_source, second_source}, 40)
            wait_for("managed viewer saves its initial state", lambda: saved_view(original),
                     lambda record: bool(record.get("view")))
            run("pane", "resize", "--pane", original["pane_id"], "--direction", "left",
                "--amount", "0.08")
            recovery_width = width(layout(original["pane_id"]), original["pane_id"])
            assert recovery_width > 40, recovery_width
            run("pane", "send-keys", original["pane_id"], "G", "k", "k", "k", "/")
            run("pane", "send-text", original["pane_id"], "History")
            before_close = wait_for(
                "persisted selection, viewport, search and manual width",
                lambda: saved_view(original),
                lambda record: record.get("width") == recovery_width
                              and record.get("view", {}).get("query") == "History"
                              and record["view"].get("searching") is True
                              and record["view"].get("selected", 0) > 20
                              and record["view"].get("top", 0) > 0)
            screen_state = visible_history(wait_for(
                "scrolled search before tab close", lambda: read_graph(original["pane_id"]),
                lambda text: bool(visible_history(text)) and "/ History" in text
                             and "Uncommitted changes" not in text))
            before_close = saved_view(original)
            previous = {item["log_id"] for item in logs()}
            run("tab", "close", second_tab)
            hooks_since(previous, event="tab.closed")
            survivors = panes(recovery_workspace)
            graphs = [pane for pane in survivors if pane.get("label") == "Git Graph Sidebar"]
            assert len(graphs) == 1, survivors
            restored = graphs[0]
            assert restored["terminal_id"] != original["terminal_id"], (original, restored)
            shared_at(restored, recovery_workspace, recovery_tab, recovery_source,
                      {recovery_source}, recovery_width)
            wait_for("restored selection, viewport and search",
                     lambda: read_graph(restored["pane_id"]),
                     lambda text: visible_history(text) == screen_state and "/ History" in text)
            after_close = wait_for("restored persistent view", lambda: saved_view(restored),
                                   lambda record: record.get("view") == before_close["view"])
            assert after_close["width"] == recovery_width, after_close
            # Late close/exit events for the old process must not clear the new
            # record. Further focus follows the replacement, not another graph.
            next_tab, next_source = create_tab(recovery_workspace, "Follow restored", base)
            focus_tabs(next_tab, recovery_tab, next_tab)
            shared_at(restored, recovery_workspace, next_tab, next_source,
                      {recovery_source, next_source}, recovery_width)
            # Constrain the focused target pane to 40 columns. A narrow tab
            # may clamp the actual split, but must not overwrite the preference.
            narrow_tab, narrow_source = create_tab(recovery_workspace, "Narrow destination", base)
            total = width(layout(narrow_source), narrow_source)
            data("pane", "split", "--pane", narrow_source, "--direction", "right",
                 "--ratio", str(40 / total), "--cwd", str(base), "--no-focus")
            focus_tabs(narrow_tab)
            narrow_actual = width(layout(restored["pane_id"]), restored["pane_id"])
            assert narrow_actual < recovery_width, (narrow_actual, recovery_width)
            wait_for("temporary clamp retains preferred width", lambda: saved_view(restored),
                     lambda record: record.get("actual_width") == narrow_actual
                                   and record.get("width") == recovery_width)
            focus_tabs(next_tab)
            shared_at(restored, recovery_workspace, next_tab, next_source,
                      {recovery_source, next_source} | {pane["pane_id"] for pane in panes(recovery_workspace)
                       if pane["tab_id"] == narrow_tab}, recovery_width)
            # Delete the narrow tab while it contains the sidebar, too: the
            # replacement must use the preference, not the clamped actual size.
            focus_tabs(narrow_tab)
            old_terminal = restored["terminal_id"]
            previous = {item["log_id"] for item in logs()}
            run("tab", "close", narrow_tab)
            hooks_since(previous, event="tab.closed")
            graphs = [pane for pane in panes(recovery_workspace)
                      if pane.get("label") == "Git Graph Sidebar"]
            assert len(graphs) == 1, graphs
            restored = graphs[0]
            assert restored["terminal_id"] != old_terminal, restored
            shared_at(restored, recovery_workspace, next_tab, next_source,
                      {recovery_source, next_source}, recovery_width)
            wait_for("view survives closing a constrained tab", lambda: saved_view(restored),
                     lambda record: record.get("view", {}).get("selected_oid") ==
                                    before_close["view"]["selected_oid"]
                                    and record["view"].get("query") == "History")
            wait_for("replacement has rendered before sending input",
                     lambda: read_graph(restored["pane_id"]),
                     lambda text: "/ History" in text and bool(visible_history(text)))
            run("pane", "send-keys", restored["pane_id"], "esc")
            wait_for("leave restored search before q", lambda: saved_view(restored),
                     lambda record: record.get("view", {}).get("searching") is False
                                   and record["view"].get("query") == "")

            # Explicit close still opts out, even when its containing tab is
            # subsequently deleted. Check both q and host pane close.
            for close_method, close_event in [("q", "pane.exited"), ("pane", "pane.closed")]:
                previous = {item["log_id"] for item in logs()}
                if close_method == "q":
                    run("pane", "send-keys", restored["pane_id"], "q")
                else:
                    run("pane", "close", restored["pane_id"])
                hooks_since(previous, event=close_event)
                wait_for("explicit close before deleting tab", lambda: panes(recovery_workspace),
                         lambda items: all(pane["terminal_id"] != restored["terminal_id"]
                                           for pane in items))
                previous = {item["log_id"] for item in logs()}
                run("tab", "close", next_tab)
                hooks_since(previous, event="tab.closed")
                assert {pane["pane_id"] for pane in panes(recovery_workspace)} == {recovery_source}
                if close_method == "q":
                    next_tab, next_source = create_tab(recovery_workspace, "Manual close", repo)
                    focus_tabs(next_tab)
                    restored = invoke("sidebar")

            # The last tab closes the workspace rather than opening elsewhere.
            second_tab, _ = create_tab(recovery_workspace, "Last graph", repo)
            focus_tabs(second_tab)
            invoke("sidebar")
            previous = {item["log_id"] for item in logs()}
            run("tab", "close", recovery_tab)
            hooks_since(previous, event="tab.closed")
            previous = {item["log_id"] for item in logs()}
            run("tab", "close", second_tab)
            hooks_since(previous, event="tab.closed")
            assert all(item["workspace_id"] != recovery_workspace
                       for item in data("workspace", "list")["workspaces"])
            assert data("pane", "get", other_sidebar["pane_id"])["pane"]["terminal_id"] == \
                other_sidebar["terminal_id"]
            # Toggle width preferences outlive the active pane registration.
            # A constrained reopen must not replace the preferred wide size.
            toggle_workspace = data("workspace", "create", "--cwd", str(repo),
                                    "--label", "Toggle width", "--focus")
            toggle_workspace_id = toggle_workspace["workspace"]["workspace_id"]
            wide_tab = toggle_workspace["tab"]["tab_id"]
            wide_source = toggle_workspace["root_pane"]["pane_id"]
            graph = invoke("toggle-sidebar")
            shared_at(graph, toggle_workspace_id, wide_tab, wide_source, {wide_source}, 40)
            run("pane", "resize", "--pane", graph["pane_id"], "--direction", "left",
                "--amount", "0.05")
            preferred = width(layout(graph["pane_id"]), graph["pane_id"])
            assert preferred > 40, preferred
            invoke_result("toggle-sidebar")
            assert {pane["pane_id"] for pane in panes(toggle_workspace_id)} == {wide_source}
            graph = invoke("toggle-sidebar")
            shared_at(graph, toggle_workspace_id, wide_tab, wide_source, {wide_source}, preferred)
            invoke_result("toggle-sidebar")

            constrained_tab, constrained_source = create_tab(toggle_workspace_id,
                                                              "Constrained toggle", repo)
            total = width(layout(constrained_source), constrained_source)
            data("pane", "split", "--pane", constrained_source, "--direction", "right",
                 "--ratio", str(40 / total), "--cwd", str(repo), "--no-focus")
            focus_tabs(constrained_tab)
            ordinary = {pane["pane_id"] for pane in panes(toggle_workspace_id)}
            graph = invoke("toggle-sidebar")
            assert width(layout(graph["pane_id"]), graph["pane_id"]) < preferred
            invoke_result("toggle-sidebar")
            focus_tabs(wide_tab)
            assert {pane["pane_id"] for pane in panes(toggle_workspace_id)} == ordinary
            graph = invoke("toggle-sidebar")
            shared_at(graph, toggle_workspace_id, wide_tab, wide_source, ordinary, preferred)
            assert data("pane", "get", other_sidebar["pane_id"])["pane"]["terminal_id"] == \
                other_sidebar["terminal_id"]
            print("PASS: isolated Herdr shared sidebar; toggle open/close/reopen and hook opt-out; "
                  "last toggle width persists, including constrained-to-wide reopen; "
                  "repeated/non-Git actions; tab moves/back/rapid focus; "
                  "stable pane/terminal, selection/scroll, manual width and source focus; "
                  "independent workspace; q/manual close opt-out; full-view action; "
                  "zoom failure/recovery; containing-tab close restores view/search/width to non-Git tab; "
                  "narrow-to-wide preferred-width recovery; "
                  "q/manual opt-out followed by tab close; last-tab close; OSC theme")
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
