#!/usr/bin/env python3
"""Optional headless Herdr sidebar topology/process regression test.

Usage: python3 tests/sidebar_layout_live.py [target/debug/herdr-git-graph]

No build is performed. Only a private, build-stripped plugin copy is linked;
HOME, XDG directories, HERDR context, socket, session, and binary are isolated
under a short /tmp path. layout.export is read over JSON-lines Unix RPC.
layout.apply is deliberately never used: it would recreate terminal processes.

This is API/headless verification, NOT visual verification in an external
terminal. Event timings describe socket arrival times, not rendered TUI frames;
older Herdr versions can buffer/throttle these events after a move completes.
"""
import argparse
from contextlib import contextmanager
from dataclasses import dataclass
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import threading
import time
import traceback


ROOT = Path(__file__).resolve().parents[1]
PLUGIN = "herdr.git-graph"


def wait_for(description, probe, ready=bool, timeout=10):
    deadline = time.monotonic() + timeout
    observed = None
    while time.monotonic() < deadline:
        observed = probe()
        if ready(observed):
            return observed
        time.sleep(0.05)
    raise AssertionError(f"Timed out waiting for {description}: {observed!r}")


def private_plugin(base, binary):
    """Copy only runtime files; never run the checkout's install/build script."""
    plugin = base / "plugin"
    (plugin / "bin").mkdir(parents=True)
    lines = []
    skipping = False
    for line in (ROOT / "herdr-plugin.toml").read_text().splitlines(keepends=True):
        header = line.strip()
        if header.startswith("["):
            skipping = header == "[[build]]" or header.startswith("[build.")
        if not skipping:
            lines.append(line)
    manifest = "".join(lines)
    assert "[[build]]" not in manifest
    # tomllib is optional so the test also runs on Python 3.9/3.10.
    try:
        import tomllib
    except ImportError:
        pass
    else:
        assert "build" not in tomllib.loads(manifest)
    (plugin / "herdr-plugin.toml").write_text(manifest)
    shutil.copy2(binary, plugin / "bin/herdr-git-graph")
    (plugin / "bin/herdr-git-graph").chmod(0o755)
    return plugin


class Events:
    """Keep every layout/move/tab event, including short-lived temporary tabs."""

    def __init__(self, path):
        self.connection = socket.socket(socket.AF_UNIX)
        self.connection.settimeout(10)
        self.connection.connect(str(path))
        self.stream = self.connection.makefile("rb")
        self.records = []
        self.error = None
        self.condition = threading.Condition()
        self.closing = False
        self.send("subscribe", "events.subscribe", {"subscriptions": [
            {"type": name} for name in
            ("layout.updated", "pane.moved", "tab.created", "tab.closed")]})
        while True:
            reply = json.loads(self.stream.readline())
            if "event" in reply:
                self.records.append((time.monotonic(), reply))
                continue
            assert reply.get("id") == "subscribe" and "error" not in reply, reply
            break
        self.connection.settimeout(None)
        self.thread = threading.Thread(target=self.receive, daemon=True)
        self.thread.start()

    def send(self, request_id, method, params):
        self.connection.sendall((json.dumps({"id": request_id, "method": method,
                                             "params": params}) + "\n").encode())

    def receive(self):
        try:
            for line in self.stream:
                message = json.loads(line)
                received = time.monotonic()
                with self.condition:
                    if "event" in message:
                        self.records.append((received, message))
                    self.condition.notify_all()
        except (OSError, ValueError) as error:
            self.error = repr(error)
        finally:
            with self.condition:
                if not self.closing and self.error is None:
                    self.error = "Subscription stream closed unexpectedly"
                self.condition.notify_all()

    def checkpoint(self):
        # events.subscribe takes over its socket: further RPC requests on that
        # connection are not supported. Actions/hooks are awaited separately;
        # Older Herdr releases deliver queued events about 100 ms apart. Wait
        # for 250 ms of quiescence so earlier actions cannot contaminate the
        # next measurement. This is not an atomic server/TUI barrier.
        deadline = time.monotonic() + 15
        with self.condition:
            while True:
                assert self.error is None, self.error
                before = len(self.records)
                changed = self.condition.wait_for(
                    lambda: len(self.records) != before or self.error is not None,
                    timeout=0.25)
                assert self.error is None, self.error
                if not changed:
                    return len(self.records)
                assert time.monotonic() < deadline, "Layout event stream did not settle"

    def close(self):
        self.closing = True
        try:
            self.connection.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass  # The server may already have closed a failed subscription.
        self.thread.join(timeout=5)
        self.stream.close()
        self.connection.close()
        assert not self.thread.is_alive(), "Event reader did not stop"


@dataclass
class TabState:
    workspace: str
    tab: str
    root: dict
    focus: str
    terminals: dict
    processes: dict


class Live:
    def __init__(self, base, herdr, binary, count):
        self.base = base
        self.count = count
        self.env = {key: value for key, value in os.environ.items()
                    if not key.startswith("HERDR_") and key != "NO_COLOR"}
        for key, folder in [("HOME", "h"), ("XDG_CONFIG_HOME", "c"),
                            ("XDG_STATE_HOME", "s"), ("XDG_DATA_HOME", "d"),
                            ("XDG_CACHE_HOME", "cache"), ("XDG_RUNTIME_DIR", "r")]:
            path = base / folder
            path.mkdir(mode=0o700, parents=True)
            self.env[key] = str(path)
        self.env.update(ZDOTDIR=str(base / "h"), GIT_CONFIG_GLOBAL=os.devnull,
                        GIT_CONFIG_NOSYSTEM="1")
        self.cli = [herdr, "--session", "layout-test"]
        self.plugin = private_plugin(base, binary)
        self.commands = []
        self.socket = None
        self.events = None
        self.server = None
        self.workspace = None
        self.known_tabs = set()

    def run(self, *args, check=True):
        started = time.monotonic()
        result = subprocess.run([*self.cli, *args], cwd=self.base, env=self.env,
                                capture_output=True, text=True, timeout=20)
        self.commands.append((args, (time.monotonic() - started) * 1000))
        if check and result.returncode:
            raise AssertionError((args, result.returncode, result.stdout, result.stderr))
        return result

    def data(self, *args):
        return json.loads(self.run(*args).stdout)["result"]

    def rpc(self, method, params):
        assert method != "layout.apply", "Never recreate pane processes in this test"
        with socket.socket(socket.AF_UNIX) as connection:
            connection.settimeout(10)
            connection.connect(str(self.socket))
            connection.sendall((json.dumps({"id": "layout-test", "method": method,
                                            "params": params}) + "\n").encode())
            with connection.makefile("rb") as stream:
                response = json.loads(stream.readline())
        assert "error" not in response, (method, response)
        return response["result"]

    def export(self, tab):
        return self.rpc("layout.export", {"tab_id": tab})["layout"]

    def panes(self):
        return self.rpc("pane.list", {"workspace_id": self.workspace})["panes"]

    def tabs(self):
        return self.rpc("tab.list", {"workspace_id": self.workspace})["tabs"]

    def logs(self):
        return self.data("plugin", "log", "list", "--plugin", PLUGIN,
                         "--limit", "500")["logs"]

    def idle(self):
        previous = None
        stable = 0

        def probe():
            nonlocal previous, stable
            logs = self.logs()
            signature = {(item["log_id"], item["status"]) for item in logs}
            stable = stable + 1 if signature == previous else 0
            previous = signature
            return logs

        return wait_for("all plugin hooks/actions to settle", probe,
                        lambda logs: stable >= 2 and all(
                            item["status"] != "running" for item in logs))

    def invoke(self, action="sidebar"):
        result = self.data("plugin", "action", "invoke", PLUGIN + "." + action)
        log_id = result["log"]["log_id"]
        log = wait_for("action completion", lambda: next(
            (item for item in self.logs() if item["log_id"] == log_id), None),
            lambda item: item is not None and item["status"] != "running")
        assert log["status"] == "succeeded", log
        self.idle()
        return json.loads(log["stdout"])["result"]

    def hook_command(self, event, *command, expected="succeeded"):
        previous = {item["log_id"] for item in self.logs()}
        self.run(*command)
        hooks = wait_for(event + " hook completion", lambda: [
            item for item in self.logs() if item.get("event") == event
            and item["log_id"] not in previous],
            lambda items: bool(items) and all(item["status"] != "running" for item in items))
        self.idle()
        for hook in hooks:
            assert hook["status"] == expected, hook
            if expected == "failed":
                assert (hook.get("stderr") or "").strip(), hook

    def process(self, pane):
        info = self.rpc("pane.process_info", {"pane_id": pane})["process_info"]
        return {"shell_pid": info.get("shell_pid"),
                "foreground_process_group_id": info.get("foreground_process_group_id"),
                "foreground": sorted((p["pid"], p["name"])
                                     for p in info.get("foreground_processes", []))}

    @staticmethod
    def sidebar_identity(snapshot):
        # A viewer periodically launches git and Herdr CLI workers in its
        # foreground process group. Those children are not persistent identity.
        mains = [pid for pid, name in snapshot["foreground"]
                 if name == "herdr-git-graph"]
        return {"main_pid": mains[0] if len(mains) == 1 else None,
                "shell_pid": snapshot["shell_pid"],
                "foreground_process_group_id": snapshot["foreground_process_group_id"]}

    def capture_sidebar_process(self, graph):
        previous = None

        def probe():
            nonlocal previous
            snapshot = self.process(graph["pane_id"])
            identity = self.sidebar_identity(snapshot)
            stable = identity == previous
            previous = identity
            return {"identity": identity, "snapshot": snapshot, "stable": stable}

        record = wait_for("stable sidebar main/shell/process-group PIDs", probe,
                          lambda state: state["stable"] and all(state["identity"].values()))
        print("SIDEBAR_PROCESS " + json.dumps({"pane_id": graph["pane_id"],
              "identity": record["identity"], "snapshot": record["snapshot"]}), flush=True)
        return record

    def assert_sidebar_process(self, graph, expected):
        snapshot = self.process(graph["pane_id"])
        identity = self.sidebar_identity(snapshot)
        diagnostic = {"pane_id": graph["pane_id"], "expected": expected,
                      "actual": {"identity": identity, "snapshot": snapshot}}
        assert identity == expected["identity"], (
            "Sidebar main/shell/process-group PID changed", diagnostic)
        if snapshot != expected["snapshot"]:
            print("PROCESS_CHILDREN_CHANGED " + json.dumps({
                **diagnostic, "persistent_identity_unchanged": True}), flush=True)

    def make_tab(self, count, label, first=False):
        if first:
            result = self.data("workspace", "create", "--cwd", str(self.base / "repo"),
                               "--label", label, "--focus")
            self.workspace = result["workspace"]["workspace_id"]
        else:
            result = self.data("tab", "create", "--workspace", self.workspace,
                               "--cwd", str(self.base / "repo"), "--label", label,
                               "--no-focus")
        tab = result["tab"]["tab_id"]
        self.known_tabs.add(tab)
        panes = [result["root_pane"]["pane_id"]]
        # Mixed directions, unequal ratios, several levels, and a non-first
        # focused leaf expose leaf-only sidebar implementations.
        plan = [(0, "down", 0.63), (0, "right", 0.57), (1, "right", 0.41),
                (2, "down", 0.45), (3, "down", 0.68), (0, "down", 0.72),
                (1, "down", 0.39)]
        for index, (target, direction, ratio) in enumerate(plan[:count - 1]):
            split = self.data("pane", "split", panes[target], "--direction", direction,
                              "--ratio", str(ratio), "--cwd", str(self.base / "repo"),
                              "--focus" if index == 0 else "--no-focus")
            panes.append(split["pane"]["pane_id"])
        for pane in panes:
            wait_for("shell PID", lambda: self.process(pane),
                     lambda state: bool(state["shell_pid"]))
            self.run("pane", "run", pane, "sleep 600")
        previous = None

        def stable_sleep():
            nonlocal previous
            current = {pane: self.process(pane) for pane in panes}
            ready = current == previous and all(
                info["shell_pid"] and any(name == "sleep" for _, name in info["foreground"])
                for info in current.values())
            previous = current
            return current if ready else None

        processes = wait_for("stable shell and foreground sleep PIDs", stable_sleep)
        exported = self.export(tab)
        terminals = {pane["pane_id"]: pane["terminal_id"] for pane in self.panes()
                     if pane["tab_id"] == tab}
        self.idle()
        return TabState(self.workspace, tab, exported["root"],
                        exported["focused_pane_id"], terminals, processes)

    def assert_processes(self, *states):
        members = {pane["pane_id"]: pane for pane in self.panes()}
        for state in states:
            assert state.tab in {tab["tab_id"] for tab in self.tabs()}, state.tab
            for pane, terminal in state.terminals.items():
                assert pane in members, (pane, members)
                assert members[pane]["terminal_id"] == terminal, (pane, members[pane])
                assert members[pane]["tab_id"] == state.tab, (pane, members[pane])
                actual = self.process(pane)
                assert actual == state.processes[pane], (pane, state.processes[pane], actual)

    def assert_tabs(self):
        actual = {tab["tab_id"] for tab in self.tabs()}
        assert actual == self.known_tabs, ("Temporary/lost tab", self.known_tabs, actual)

    def assert_original(self, state):
        exported = self.export(state.tab)
        assert exported["root"] == state.root, ("Original tree/ratios changed", state.root, exported)
        assert exported["focused_pane_id"] == state.focus, (state.focus, exported)
        assert {pane["pane_id"] for pane in self.panes() if pane["tab_id"] == state.tab} == \
            set(state.terminals)

    def assert_sidebar(self, state, graph):
        exported = self.export(state.tab)
        root = exported["root"]
        assert root["type"] == "split" and root["direction"] == "right", exported
        assert root["second"]["type"] == "pane", exported
        assert root["second"]["pane_id"] == graph["pane_id"], exported
        assert root["first"] == state.root, ("Original tree must be the entire left subtree",
                                             state.root, exported)
        assert exported["focused_pane_id"] == state.focus, (state.focus, exported)
        members = self.panes()
        current = next(pane for pane in members if pane["pane_id"] == graph["pane_id"])
        assert current["terminal_id"] == graph["terminal_id"], (graph, current)
        assert current["tab_id"] == state.tab and not current["focused"], current
        assert {pane["pane_id"] for pane in members if pane["tab_id"] == state.tab} == \
            set(state.terminals) | {graph["pane_id"]}, members
        snapshot = self.rpc("pane.layout", {"pane_id": graph["pane_id"]})["layout"]
        rect = next(pane["rect"] for pane in snapshot["panes"]
                    if pane["pane_id"] == graph["pane_id"])
        area = snapshot["area"]
        assert rect["width"] == 40, snapshot
        assert rect["y"] == area["y"] and rect["height"] == area["height"], snapshot
        assert rect["x"] + rect["width"] == area["x"] + area["width"], snapshot
        assert not snapshot["zoomed"], snapshot
        self.assert_tabs()

    def rendered(self, graph):
        wait_for("sidebar rendering", lambda: self.run(
            "pane", "read", graph["pane_id"], "--source", "visible", "--raw").stdout,
            lambda text: "GIT GRAPH" in text)

    @contextmanager
    def measure(self, label, no_rearrangement=False):
        start_index = self.events.checkpoint()
        command_index = len(self.commands)
        started = time.monotonic()
        try:
            yield
        finally:
            completed = time.monotonic()
            elapsed = (completed - started) * 1000
            end_index = self.events.checkpoint()
            drain_ms = (time.monotonic() - completed) * 1000
            records = self.events.records[start_index:end_index]
            created, closed = {}, {}
            for received, message in records:
                data = message["data"]
                if message["event"] == "tab_created":
                    created[data["tab"]["tab_id"]] = received
                elif message["event"] == "tab_closed":
                    closed[data["tab_id"]] = received
                elif message["event"] == "pane_moved" and data.get("created_tab"):
                    created.setdefault(data["created_tab"]["tab_id"], received)
            moves = sum(message["event"] == "pane_moved" for _, message in records)
            layouts = [(received, message["data"]["layout"]) for received, message in records
                       if message["event"] == "layout_updated"]
            commands = self.commands[command_index:]
            triggers = [(" ".join(args), round(duration, 3)) for args, duration in commands
                        if args[:3] == ("plugin", "action", "invoke")
                        or args[:2] in (("tab", "focus"), ("tab", "close"),
                                       ("pane", "close"), ("pane", "zoom"))]
            print("TIMING " + json.dumps({
                "case_panes": self.count, "operation": label,
                "completion_wall_ms": round(elapsed, 3), "actual_cli_wall_ms": triggers,
                "event_drain_wall_ms": round(drain_ms, 3),
                "event_timing_basis": "socket_arrival_not_ui_buffering_possible",
                "layout_updated_events": len(layouts), "pane_moved_events": moves,
                "layout_event_span_ms": round((layouts[-1][0] - layouts[0][0]) * 1000, 3)
                if len(layouts) > 1 else 0,
                "temporary_tab_arrival_windows": [
                    {"tab_id": tab, "closed": tab in closed,
                     "observed_ms": round((closed[tab] - received) * 1000, 3)
                     if tab in closed else None}
                    for tab, received in created.items() if tab not in self.known_tabs],
                "headless_api_only": True}), flush=True)
            if no_rearrangement:
                assert not moves and not created and not closed and not layouts, (
                    "Same-tab invocation rearranged layout", records)

    def scenario(self):
        first = self.make_tab(self.count, f"Nested {self.count}", first=True)
        # Finish both ordinary trees before registering a sidebar. Otherwise a
        # focused fixture split could trigger sidebar migration during setup.
        second = self.make_tab(3 if self.count == 3 else 6, "Nested destination")
        self.hook_command("tab.focused", "tab", "focus", first.tab)
        self.assert_original(first)
        self.assert_original(second)
        self.assert_processes(first, second)
        self.events = Events(self.socket)
        print(f"CASE {self.count}: shell/sleep identities " +
              json.dumps(first.processes, sort_keys=True), flush=True)
        with self.measure("first open"):
            graph = self.invoke()["plugin_pane"]["pane"]
        self.assert_processes(first, second)
        self.assert_sidebar(first, graph)
        self.assert_original(second)
        self.rendered(graph)
        graph_process = self.capture_sidebar_process(graph)
        with self.measure("same-tab reinvocation", no_rearrangement=True):
            repeated = self.invoke()["plugin_pane"]["pane"]
        assert repeated["pane_id"] == graph["pane_id"], (graph, repeated)
        assert repeated["terminal_id"] == graph["terminal_id"], (graph, repeated)
        self.assert_sidebar(first, graph)
        self.assert_processes(first, second)
        self.assert_sidebar_process(graph, graph_process)

        for destination, origin in [(second, first), (first, second), (second, first)]:
            with self.measure("tab move to " + destination.tab):
                self.hook_command("tab.focused", "tab", "focus", destination.tab)
            self.assert_sidebar(destination, graph)
            self.assert_original(origin)
            self.assert_processes(first, second)
            self.assert_sidebar_process(graph, graph_process)
        before = self.export(second.tab)
        with self.measure("destination reinvocation", no_rearrangement=True):
            repeated = self.invoke()["plugin_pane"]["pane"]
        assert repeated["pane_id"] == graph["pane_id"], repeated
        assert self.export(second.tab) == before
        self.assert_sidebar_process(graph, graph_process)

        with self.measure("toggle close restores nested tree"):
            self.invoke("toggle-sidebar")
        self.assert_original(first)
        self.assert_original(second)
        self.assert_processes(first, second)
        self.assert_tabs()
        for state in (first, second):
            self.hook_command("tab.focused", "tab", "focus", state.tab)
        assert {pane["pane_id"] for pane in self.panes()} == \
            set(first.terminals) | set(second.terminals), self.panes()
        with self.measure("reopen after toggle"):
            graph = self.invoke()["plugin_pane"]["pane"]
        self.assert_sidebar(second, graph)
        self.rendered(graph)
        graph_process = self.capture_sidebar_process(graph)

        # A zoomed source must remain untouched when pane.move refuses. Do not
        # silently unzoom the user's tab or leave partially evacuated panes.
        self.run("pane", "zoom", "--pane", second.focus, "--on")
        zoomed_tree = self.export(second.tab)
        with self.measure("zoomed-source move failure"):
            self.hook_command("tab.focused", "tab", "focus", first.tab, expected="failed")
        assert self.export(second.tab) == zoomed_tree
        assert self.rpc("pane.layout", {"pane_id": second.focus})["layout"]["zoomed"]
        stranded = self.rpc("pane.get", {"pane_id": graph["pane_id"]})["pane"]
        assert stranded["tab_id"] == second.tab and stranded["terminal_id"] == graph["terminal_id"]
        self.assert_original(first)
        self.assert_processes(first, second)
        self.assert_tabs()
        self.assert_sidebar_process(graph, graph_process)
        self.run("pane", "zoom", "--pane", second.focus, "--off")
        # zoom --off focuses its addressed source tab; move away and back.
        for destination, origin in [(first, second), (second, first)]:
            with self.measure("unzoomed recovery to " + destination.tab):
                self.hook_command("tab.focused", "tab", "focus", destination.tab)
            self.assert_sidebar(destination, graph)
            self.assert_original(origin)
            self.assert_processes(first, second)
            self.assert_sidebar_process(graph, graph_process)

        records = self.base / "s/herdr/plugins/herdr.git-graph/shared-sidebars"

        def saved_view():
            for path in records.glob("*.json"):
                try:
                    record = json.loads(path.read_text())
                except (FileNotFoundError, json.JSONDecodeError):
                    continue
                if record.get("pane_id") == graph["pane_id"] and record.get("view"):
                    return record
            return None

        wait_for("persisted graph before containing-tab close", saved_view)
        old_terminal = graph["terminal_id"]
        self.known_tabs.remove(second.tab)
        with self.measure("containing-tab close and recovery"):
            self.hook_command("tab.closed", "tab", "close", second.tab)
        graphs = [pane for pane in self.panes() if pane.get("label") == "Git Graph Sidebar"]
        assert len(graphs) == 1, self.panes()
        graph = graphs[0]
        assert graph["terminal_id"] != old_terminal, graph
        self.assert_sidebar(first, graph)
        self.assert_processes(first)
        self.rendered(graph)
        with self.measure("direct pane close restores nested tree"):
            self.hook_command("pane.closed", "pane", "close", graph["pane_id"])
        self.assert_original(first)
        self.assert_processes(first)
        self.assert_tabs()
        assert {pane["pane_id"] for pane in self.panes()} == set(first.terminals)
        print(f"PASS: {self.count}-pane nested layout; full-height root-right width 40; "
              "original ratios/tree/tab/pane/terminal/shell/sleep identity and focus; "
              "same-tab no rearrangement; nested tab moves; toggle/direct close; "
              "zoom failure/recovery; containing-tab recovery; no temporary tabs remain. "
              "Headless API verification only, not external-terminal visual QA.", flush=True)

    def verify(self):
        repo = self.base / "repo"
        repo.mkdir()
        for args in [("init",), ("config", "user.name", "Layout test"),
                     ("config", "user.email", "layout@example.invalid")]:
            subprocess.run(["git", "-C", str(repo), *args], env=self.env,
                           check=True, capture_output=True)
        (repo / "example.txt").write_text("layout fixture\n")
        subprocess.run(["git", "-C", str(repo), "add", "."], env=self.env,
                       check=True, capture_output=True)
        subprocess.run(["git", "-C", str(repo), "-c", "commit.gpgsign=false", "commit",
                        "-m", "Layout fixture"], env=self.env, check=True, capture_output=True)
        with (self.base / "server.log").open("w") as output:
            self.server = subprocess.Popen([*self.cli, "server"], cwd=self.base,
                                           env=self.env, stdout=output, stderr=output)
            try:
                def socket_path():
                    assert self.server.poll() is None, (self.base / "server.log").read_text()
                    # Older releases also create herdr-client.sock. Only the
                    # server API socket accepts these JSON-lines requests.
                    paths = list(self.base.rglob("herdr.sock"))
                    assert len(paths) <= 1, paths
                    return paths[0] if paths else None

                self.socket = wait_for("isolated Herdr socket", socket_path, timeout=5)
                wait_for("isolated server startup", lambda: self.run(
                    "status", "server", check=False).stdout,
                    lambda text: "status: running" in text, timeout=5)
                print("HERDR: " + self.run("--version").stdout.strip(), flush=True)
                schema = json.loads(self.run("api", "schema", "--json").stdout)
                subscriptions = schema["schemas"]["request"]["$defs"]["Subscription"]["oneOf"]
                supported = {item["properties"]["type"]["const"] for item in subscriptions}
                assert "layout.updated" in supported, supported
                print("SCHEMA: events.subscribe supports layout.updated; "
                      "collecting layout_updated/pane_moved/tab_created/tab_closed events; "
                      "timings are socket arrivals, not TUI frame timings.", flush=True)
                self.run("plugin", "link", str(self.plugin))
                self.scenario()
            finally:
                try:
                    if self.events:
                        self.events.close()
                finally:
                    try:
                        self.run("server", "stop", check=False)
                    finally:
                        try:
                            self.server.wait(timeout=5)
                        except subprocess.TimeoutExpired:
                            # Only our isolated child, never a personal server.
                            self.server.terminate()
                            self.server.wait(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", nargs="?", type=Path,
                        default=ROOT / "target/debug/herdr-git-graph")
    parser.add_argument("--panes", type=int, choices=(3, 8), action="append",
                        help="Run just this fixture (repeatable); default: both 3 and 8")
    args = parser.parse_args()
    binary = args.binary.resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK):
        parser.error(f"Executable not found: {binary}; build it separately (this test never builds)")
    herdr = shutil.which("herdr")
    if not herdr:
        parser.error("Install Herdr before running this optional integration test")
    failures = []
    for count in args.panes or (3, 8):
        with tempfile.TemporaryDirectory(prefix="hgl-", dir="/tmp") as temporary:
            try:
                Live(Path(temporary), herdr, binary, count).verify()
            except Exception:
                failures.append(count)
                print(f"FAIL: {count}-pane fixture", flush=True)
                traceback.print_exc()
    if failures:
        raise SystemExit(f"Failed nested sidebar fixtures: {failures}")


if __name__ == "__main__":
    main()
