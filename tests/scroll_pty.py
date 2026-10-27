#!/usr/bin/env python3
"""Long-history wheel regressions on a real, isolated Unix controlling PTY.

    cargo build
    python3 tests/scroll_pty.py [target/debug/herdr-git-graph]
    HERDR_GIT_GRAPH_TEST_BIN=/absolute/path/to/binary python3 tests/scroll_pty.py

Python 3 + the standard library and Git are sufficient; no Herdr installation or
server is used. Kitty output is captured, not displayed. The frame-count bound
is deliberately generous: this checks coalescing, not a machine-speed ratio.
"""

import argparse
import errno
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import shutil
import struct
import subprocess
import tempfile
import termios
import time
import unittest


ROOT = Path(__file__).resolve().parents[1]
COMMITS = 2400
ROWS, COLUMNS = 24, 64
HISTORY_HEIGHT = (ROWS - 6) // 2 * 2
PANE = "scroll-pty-fake-pane"
WORKSPACE = "scroll-pty-fake-workspace"
WHEEL_DOWN = b"\x1b[<65;12;7M"
WHEEL_UP = b"\x1b[<64;12;7M"
MOUSE_MOVED = b"\x1b[<35;12;7M"
CLICK = b"\x1b[<0;12;7M\x1b[<0;12;7m"
KITTY = re.compile(rb"\x1b_G([^;]*);.*?\x1b\\", re.DOTALL)
TIMEOUT = 20
PERSIST_IDLE = 0.4  # Longer than the shared-view persistence throttle (250 ms).


def isolated_env(base):
    """Never inherit a caller's pane, socket, plugin context, or Git config."""
    env = {key: value for key, value in os.environ.items()
           if not key.startswith(("HERDR_", "GIT_"))
           and key not in {"NO_COLOR", "CLICOLOR_FORCE"}}
    for key, folder in [("HOME", "home"), ("XDG_CONFIG_HOME", "config"),
                        ("XDG_STATE_HOME", "state"), ("XDG_DATA_HOME", "data"),
                        ("XDG_CACHE_HOME", "cache")]:
        directory = base / folder
        directory.mkdir(parents=True, exist_ok=True)
        env[key] = str(directory)
    env.update({
        "TERM": "xterm-256color",
        "COLORTERM": "truecolor",
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_TERMINAL_PROMPT": "0",
        "HERDR_ENV": "1",
        "HERDR_WORKSPACE_ID": WORKSPACE,
        "HERDR_PANE_ID": PANE,
        "HERDR_TERMINAL_ID": "scroll-pty-fake-terminal",
        "HERDR_TAB_ID": "scroll-pty-fake-tab",
        "HERDR_PLUGIN_ID": "herdr.git-graph",
        "HERDR_PLUGIN_ROOT": str(base / "not-a-plugin-install"),
        "HERDR_PLUGIN_STATE_DIR": str(base / "plugin-state"),
        "HERDR_SOCKET_PATH": str(base / "no-server.sock"),
        # Geometry probes may try to exec this path, but cannot reach a real
        # binary or server, even if `herdr` is on the user's PATH.
        "HERDR_BIN_PATH": str(base / "no-herdr-binary"),
        "HERDR_GIT_GRAPH_SHARED": "1",
    })
    return env


def make_repository(base):
    """One fast-import, not thousands of git-commit subprocesses or hooks."""
    repo = base / "repo"
    env = isolated_env(base / "git-env")
    subprocess.run(["git", "-c", "init.defaultBranch=main", "init", "--quiet", str(repo)],
                   env=env, check=True, capture_output=True, timeout=TIMEOUT)
    commands = []
    for index in range(COMMITS):
        subject = f"PTY history {index:04d}\n"
        commands.append(
            f"commit refs/heads/main\nmark :{index + 1}\n"
            f"committer Graph PTY <pty@example.invalid> {1700000000 + index} +0000\n"
            f"data {len(subject.encode())}\n{subject}"
            + (f"from :{index}\n" if index else "") + "\n")
    commands.append("done\n")
    subprocess.run(["git", "-C", str(repo), "fast-import", "--quiet"],
                   input="".join(commands).encode(), env=env, check=True,
                   capture_output=True, timeout=TIMEOUT)
    result = subprocess.run(["git", "-C", str(repo), "rev-list", "HEAD"], env=env,
                            check=True, capture_output=True, text=True, timeout=TIMEOUT)
    oids = result.stdout.splitlines()
    assert len(oids) == COMMITS, len(oids)
    return repo, oids


def upload_headers(output):
    """Count upload *starts*, not Kitty's 4096-byte continuation chunks."""
    headers = []
    for match in KITTY.finditer(output):
        fields = dict(part.split(b"=", 1) for part in match[1].split(b",") if b"=" in part)
        if fields.get(b"a") == b"T":
            headers.append(fields)
    return headers


def controlling_terminal():
    # Popen has already dup2'ed the slave onto stdin. setsid alone does not
    # acquire a controlling terminal on either Linux or macOS.
    os.setsid()
    fcntl.ioctl(0, termios.TIOCSCTTY, 0)


class GraphPTY:
    def __init__(self, binary, repo, env, theme="classic"):
        self.master, self.slave = pty.openpty()
        self.process = None
        self.closed = False
        self.output = bytearray()
        self.pending = bytearray()
        self.dsr_scan = 0
        self.original_termios = termios.tcgetattr(self.slave)
        self.resize(ROWS, COLUMNS)
        os.set_blocking(self.master, False)
        try:
            self.process = subprocess.Popen(
                [str(binary), "--repo", str(repo), "--sidebar", "--renderer", "curves",
                 "--theme", theme, "--limit", str(COMMITS), "--no-auto-refresh"],
                cwd=repo, env=env, stdin=self.slave, stdout=self.slave, stderr=self.slave,
                close_fds=True, preexec_fn=controlling_terminal)
        except BaseException:
            self.close()
            raise

    def resize(self, rows, columns):
        # Realistic pixel dimensions also exercise TIOCGWINSZ curve raster sizing.
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ,
                    struct.pack("HHHH", rows, columns, columns * 10, rows * 20))

    def diagnostic(self):
        # Suppress base64 PNGs in a failure while retaining ANSI/text diagnostics.
        tail = KITTY.sub(b"<Kitty>", bytes(self.output))[-2500:]
        return f"pid={self.process.pid}, exit={self.process.poll()}, output tail={tail!r}"

    def pump(self, timeout=0.02):
        """Drain output while writing, handling short writes and backpressure."""
        readable, writable, _ = select.select(
            [self.master], [self.master] if self.pending else [], [], timeout)
        if readable:
            try:
                chunk = os.read(self.master, 65536)
            except (BlockingIOError, InterruptedError):
                chunk = b""
            except OSError as error:
                if error.errno != errno.EIO:
                    raise
                chunk = b""
            self.output.extend(chunk)
            # Some terminal libraries request the cursor position at startup.
            # Keep the last three bytes in the scan to recognize split requests.
            end = len(self.output)
            for _ in re.finditer(rb"\x1b\[6n", self.output[self.dsr_scan:]):
                self.pending.extend(b"\x1b[1;1R")
            self.dsr_scan = max(self.dsr_scan, end - 3)
        if writable and self.pending:
            try:
                written = os.write(self.master, self.pending[:65536])
            except (BlockingIOError, InterruptedError):
                return
            if written <= 0:
                raise AssertionError("PTY input write made no progress")
            del self.pending[:written]

    def wait_for(self, description, probe, timeout=TIMEOUT):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            observed = probe()
            if observed:
                return observed
            if self.process.poll() is not None:
                self.idle(0.05)
                raise AssertionError(f"Exited waiting for {description}: {self.diagnostic()}")
            self.pump()
        raise AssertionError(f"Timed out waiting for {description}: {self.diagnostic()}")

    def send(self, payload):
        self.pending.extend(payload)
        self.wait_for("nonblocking input payload to drain", lambda: not self.pending)

    def idle(self, seconds=PERSIST_IDLE):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            self.pump(min(0.02, max(0, deadline - time.monotonic())))

    def quit(self):
        if self.process.poll() is None:
            self.send(b"q")
        deadline = time.monotonic() + TIMEOUT
        while self.process.poll() is None and time.monotonic() < deadline:
            self.pump()
        if self.process.poll() is None:
            raise AssertionError(f"Quit did not respond: {self.diagnostic()}")
        self.idle(0.1)
        if self.process.returncode != 0:
            raise AssertionError(f"Unclean exit: {self.diagnostic()}")
        output = bytes(self.output)
        for sequence in [b"\x1b[?1049h", b"\x1b[?1049l", b"\x1b[?25h"]:
            if sequence not in output:
                raise AssertionError(f"Missing terminal lifecycle sequence {sequence!r}")
        modes = re.findall(rb"\x1b\[\?(1000|1002|1003|1006|1015)h", output)
        if not modes:
            raise AssertionError("Mouse capture was not enabled")
        for mode in modes:
            if b"\x1b[?" + mode + b"l" not in output:
                raise AssertionError(f"Mouse capture {mode!r} was not disabled")
        if upload_headers(output):
            deletion = f"\x1b_Ga=d,d=I,i={self.process.pid},q=2;\x1b\\".encode()
            if deletion not in output:
                raise AssertionError("Quit leaked the process's Kitty image")
        # macOS revokes the old slave fd when its controlling session exits;
        # the master still exposes the restored terminal attributes afterward.
        if termios.tcgetattr(self.master) != self.original_termios:
            raise AssertionError("Quit did not restore the PTY's termios/raw mode")

    def close(self):
        if self.closed:
            return
        self.closed = True
        if self.process is not None and self.process.poll() is None:
            # Failure cleanup must also work with an incomplete OSC/input stream.
            self.process.terminate()
            deadline = time.monotonic() + 2
            while self.process.poll() is None and time.monotonic() < deadline:
                self.pump()
            if self.process.poll() is None:
                self.process.kill()
            self.process.wait(timeout=5)
        os.close(self.master)
        os.close(self.slave)


class ScrollPTYTests(unittest.TestCase):
    # Filled by main so running the file never requires a server or build step.
    binary = None
    repo = None
    oids = None

    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="hgg-scroll-case-")
        self.addCleanup(temporary.cleanup)
        self.base = Path(temporary.name)
        self.env = isolated_env(self.base)
        directory = Path(self.env["HERDR_PLUGIN_STATE_DIR"]) / "shared-sidebars"

        # Discover Rust's opaque DefaultHasher identity rather than reimplement
        # it in Python. An unregistered viewer creates only its .lock marker;
        # seed the matching record *after* that bootstrap has exited. No Herdr
        # actions, server, or race against the registration reader are needed.
        bootstrap = GraphPTY(self.binary, self.repo, self.env)
        self.addCleanup(bootstrap.close)
        bootstrap.wait_for("bootstrap graph and lock marker", lambda:
                           b"GIT GRAPH" in bootstrap.output
                           and upload_headers(bootstrap.output)
                           and list(directory.glob("*.lock")))
        bootstrap.quit()
        bootstrap.close()
        markers = list(directory.glob("*.lock"))
        self.assertEqual(len(markers), 1, markers)
        self.record_path = markers[0].with_suffix(".json")
        self.record_path.write_text(json.dumps({
            "pane_id": PANE,
            "terminal_id": self.env["HERDR_TERMINAL_ID"],
            "tab_id": self.env["HERDR_TAB_ID"],
            "repo": str(self.repo),
            "test_marker": "private-scroll-pty",
            # Incomplete on purpose: a real snapshot adds top_oid, proving that
            # the second process actually restored/updated this registration.
            "view": {"selected": None, "top": 0, "top_offset": 0},
        }))

    def read_view(self):
        try:
            record = json.loads(self.record_path.read_text())
        except (FileNotFoundError, json.JSONDecodeError):
            return {}
        self.assertEqual(record.get("pane_id"), PANE, record)
        self.assertEqual(record.get("test_marker"), "private-scroll-pty", record)
        return record.get("view", {})

    def start(self, theme="classic"):
        self.graph = GraphPTY(self.binary, self.repo, self.env, theme)
        self.addCleanup(self.graph.close)
        self.graph.wait_for("registered initial snapshot", lambda:
                            "top_oid" in self.read_view()
                            and b"GIT GRAPH" in self.graph.output)
        if theme == "classic":
            self.graph.wait_for("initial Kitty PNG upload", lambda:
                                upload_headers(self.graph.output))
        self.graph.idle()
        self.assert_position(0)
        return self.graph

    def assert_position(self, row, selected=None):
        """Read the registration while alive: normal quit deletes this JSON."""
        def matches():
            view = self.read_view()
            return (view.get("top") == row // 2
                    and view.get("top_offset") == row % 2
                    and view.get("selected") == selected
                    and view.get("top_oid") == self.oids[row // 2]
                    and view.get("selected_oid") == (
                        None if selected is None else self.oids[selected]))

        self.graph.wait_for(f"persisted row={row}, selected={selected}", matches)
        self.graph.idle()  # Include the trailing 250 ms throttled state flush.
        self.assertTrue(matches(), f"Unstable persisted snapshot: {self.read_view()}")
        return self.read_view()

    def finish(self):
        self.graph.quit()
        self.assertFalse(self.record_path.exists(), "Quit kept the shared registration")
        self.graph.close()

    def test_burst_has_bounded_curve_uploads(self):
        graph = self.start()
        initial = upload_headers(graph.output)
        self.assertEqual(initial[-1][b"f"], b"100", "Expected Kitty PNG curves")
        self.assertEqual(int(initial[-1][b"r"]), HISTORY_HEIGHT)
        mark = len(graph.output)
        events = 512
        graph.send(WHEEL_DOWN * events)
        final_view = self.assert_position(events * 3)
        frames = len(upload_headers(graph.output[mark:]))
        # At least one trailing frame must reach the final state, but rendering
        # every event (512 uploads) or even every other event is a regression.
        self.assertGreaterEqual(frames, 1)
        self.assertLessEqual(frames, 64, f"{frames} Kitty uploads for {events} wheel events")
        print(f"\n  burst: {events} wheels -> {frames} PNG uploads; "
              f"top={final_view['top']}, offset={final_view['top_offset']}", flush=True)
        self.finish()

    def test_burst_crosses_input_buffer_without_followup_input(self):
        graph = self.start()
        # 1,408 bytes: larger than Crossterm's 1 KiB read buffer, but small
        # enough for one PTY write. No later input should be needed to wake it.
        events = 128
        graph.send(WHEEL_DOWN * events)
        self.assert_position(events * 3)
        self.finish()

    def test_continuous_wheels_paint_before_the_stream_stops(self):
        graph = self.start()
        mark = len(graph.output)
        events = 64
        for _ in range(events):
            graph.pending.extend(WHEEL_DOWN)
            graph.idle(0.008)
        graph.send(b"")
        during_input = len(upload_headers(graph.output[mark:]))
        self.assertGreaterEqual(during_input, 4,
                                "Continuous wheel input postponed drawing until idle")
        self.assert_position(events * 3)
        self.finish()

    def test_motion_events_do_not_break_scroll_batches(self):
        graph = self.start()
        mark = len(graph.output)
        events = 256
        graph.send((WHEEL_DOWN + MOUSE_MOVED) * events)
        self.assert_position(events * 3)
        frames = len(upload_headers(graph.output[mark:]))
        self.assertGreaterEqual(frames, 1)
        self.assertLessEqual(frames, 32, f"Ignored pointer motion forced {frames} uploads")
        self.finish()

    def test_direction_changes_and_boundary_clamps_preserve_order(self):
        graph = self.start()
        position = 0
        # Clamping *each* event differs from clamping a summed delta, dropping
        # intermediate direction changes, or applying only the last wheel.
        for groups in [[(WHEEL_UP, 48), (WHEEL_DOWN, 20), (WHEEL_UP, 37), (WHEEL_DOWN, 11)],
                       [(WHEEL_DOWN, 73), (WHEEL_UP, 29), (WHEEL_DOWN, 8), (WHEEL_UP, 11)],
                       [(WHEEL_DOWN, COMMITS * 2 // 3 + 32),
                        (WHEEL_UP, 7), (WHEEL_DOWN, 4)]]:
            payload = bytearray()
            for wheel, count in groups:
                payload.extend(wheel * count)
                for _ in range(count):
                    position = min(COMMITS * 2 - HISTORY_HEIGHT,
                                   max(0, position + (3 if wheel == WHEEL_DOWN else -3)))
            graph.send(payload)
            self.assert_position(position)
        self.finish()

    def test_click_after_pending_wheels_uses_newly_rendered_rows(self):
        graph = self.start()
        # Body begins at zero-based y=3. The click at y=6 is body row 3;
        # after 17 wheels its commit is (51 + 3) // 2 = 27, not the old row 1.
        # Keep wheel and click in ONE payload, with no artificial frame wait.
        graph.send(WHEEL_DOWN * 17 + CLICK)
        self.assert_position(51, selected=27)
        graph.send(b"j")
        self.assert_position(51, selected=28)
        self.finish()

    def test_keyboard_resize_and_quit_behind_wheels(self):
        graph = self.start()
        graph.send(WHEEL_DOWN * 9 + b"\x1b[6~")  # PageDown after the wheel frame.
        position = 27 + HISTORY_HEIGHT
        self.assert_position(position)
        graph.send(b"j")  # With no selection, j selects the viewport's top.
        position -= position % 2  # Keeping that selected row visible removes offset 1.
        self.assert_position(position, selected=position // 2)

        mark = len(graph.output)
        resized_rows = 32
        graph.resize(resized_rows, 72)
        graph.wait_for("resize to repaint Kitty placement", lambda:
                       any(int(header[b"r"]) == resized_rows - 6
                           for header in upload_headers(graph.output[mark:])))
        self.assert_position(position, selected=position // 2)
        # q must not be starved behind a pending wheel frame. Snapshot above is
        # intentionally taken before q removes the shared registration.
        mark = len(graph.output)
        graph.send(WHEEL_DOWN * 64 + b"q")
        # Input already includes q; quit() would enqueue an unnecessary key.
        graph.wait_for("quit behind pending wheels", lambda: graph.process.poll() is not None)
        graph.quit()
        frames = len(upload_headers(graph.output[mark:]))
        self.assertGreaterEqual(frames, 1, "q skipped the pending wheel frame")
        self.assertLessEqual(frames, 16)
        self.assertFalse(self.record_path.exists())
        graph.close()

    def test_ui_keeps_responding_while_shared_state_is_locked(self):
        graph = self.start()
        # Model a hook holding the exact shared lock on an independent fd.
        # Startup/restore and opt-out/quit are intentionally blocking; ordinary
        # persistence must try_lock instead, without stalling wheel or key UI.
        with self.record_path.with_suffix(".lock").open("r+b") as held:
            fcntl.flock(held, fcntl.LOCK_EX | fcntl.LOCK_NB)
            try:
                mark = len(graph.output)
                graph.send(WHEEL_DOWN * 64 + b"?")
                graph.wait_for("help key to repaint while shared lock is held", lambda:
                               b"close help" in graph.output[mark:], timeout=5)
                mark = len(graph.output)
                graph.send(b"?")  # Dismiss help, not quit; lock is still held.
                graph.wait_for("graph repaint while shared lock is held", lambda:
                               upload_headers(graph.output[mark:]), timeout=5)
                self.assertEqual(self.read_view().get("top"), 0,
                                 "State changed despite the hook holding its lock")
            finally:
                fcntl.flock(held, fcntl.LOCK_UN)
        self.assert_position(64 * 3)
        self.finish()  # Release the lock before q's intentionally blocking close.

    def test_auto_osc_palette_replies_interleave_with_wheels(self):
        graph = self.start(theme="auto")
        graph.wait_for("OSC palette queries", lambda:
                       b"\x1b]10;?" in graph.output and b"\x1b]4;6;?" in graph.output)
        mark = len(graph.output)
        # Supply valid terminal replies, including both ST and BEL terminators.
        # Mouse events may occur while the color parser has a fragmented OSC
        # body; they must remain ordered input, not disappear into that body.
        payload = (b"\x1b]10;rgb:dddd/eeee/ffff\x1b\\" + WHEEL_DOWN * 16
                   + b"\x1b]11;rgb:1111/" + WHEEL_DOWN * 16
                   + b"2222/3333\x07")
        for index, color in enumerate([b"aa/22/33", b"44/aa/55", b"bb/aa/66",
                                       b"44/66/bb", b"99/55/aa", b"33/aa/bb"], 1):
            payload += f"\x1b]4;{index};rgb:".encode() + color + b"\x1b\\" + WHEEL_DOWN * 8
        # Z cannot match the fixture's subjects, author, refs, or hex OIDs, so
        # incremental search cannot move the viewport or create a selection.
        payload += WHEEL_UP * 7 + b"/Z_PTY"
        graph.send(payload)
        self.assert_position((16 + 16 + 6 * 8 - 7) * 3)
        graph.wait_for("auto theme to enable curve uploads", lambda:
                       upload_headers(graph.output[mark:]))
        view = self.read_view()
        self.assertEqual(view.get("query"), "Z_PTY", "OSC reply bytes leaked into keyboard input")
        self.assertIs(view.get("searching"), True)
        frames = len(upload_headers(graph.output[mark:]))
        self.assertLessEqual(frames, 32, f"Auto theme rendered {frames} uploads for 87 wheels")
        # Esc closes search; q then quits. Sending q while searching types q.
        graph.send(b"\x1b")
        graph.wait_for("Escape to leave search", lambda: self.read_view().get("searching") is False)
        self.finish()


def main():
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("binary", nargs="?", default=os.environ.get(
        "HERDR_GIT_GRAPH_TEST_BIN", str(ROOT / "target/debug/herdr-git-graph")))
    args = parser.parse_args()
    binary = Path(args.binary).expanduser().resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK):
        parser.error(f"Executable not found: {binary}; run cargo build or pass a binary path")
    if not shutil.which("git"):
        parser.error("Git is required for the private fast-import fixture")
    with tempfile.TemporaryDirectory(prefix="hgg-scroll-pty-") as temporary:
        repo, oids = make_repository(Path(temporary))
        ScrollPTYTests.binary = binary
        ScrollPTYTests.repo = repo
        ScrollPTYTests.oids = oids
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(ScrollPTYTests)
        result = unittest.TextTestRunner(verbosity=2).run(suite)
    return 0 if result.wasSuccessful() else 1


if __name__ == "__main__":
    raise SystemExit(main())
