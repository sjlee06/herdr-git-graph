#!/usr/bin/env python3
"""PTY and Herdr socket smoke tests. Python standard library only; macOS/Linux."""
import argparse
import atexit
import codecs
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import signal
import socket
import struct
import subprocess
import tempfile
import termios
import threading
import time
import unicodedata


DARK_COLORS = {"10": "dddd/eeee/ffff", "11": "1212/1818/2020",
               "4;1": "dddd/4444/5555", "4;2": "4444/bbbb/7777",
               "4;3": "cccc/9999/3333", "4;4": "4444/8888/cccc",
               "4;5": "aaaa/6666/cccc", "4;6": "3333/bbbb/aaaa"}
LIGHT_COLORS = dict(DARK_COLORS, **{"10": "2222/3333/4444", "11": "fafa/fafa/fafa"})


class Terminal:
    def __init__(self, binary, extra=(), env=None, size=(140, 44), demo=True, colors=None):
        self.master, self.slave = pty.openpty()
        self.original = termios.tcgetattr(self.slave)
        self.output = bytearray()
        self.screen = {}
        self.cursor = (0, 0)
        self.escape = ""
        self.decoder = codecs.getincrementaldecoder("utf-8")("replace")
        self.colors = colors
        self.queries = bytearray()
        self.query_count = 0
        self.resize(*size)
        child_env = dict(os.environ, TERM="xterm-256color")
        for key in ["HERDR_SOCKET_PATH", "HERDR_PANE_ID", "HERDR_GIT_GRAPH_THEME", "NO_COLOR"]:
            child_env.pop(key, None)
        child_env.update(env or {})
        self.process = subprocess.Popen(
            [str(binary), *(["--demo"] if demo else []), *extra], stdin=self.slave,
            stdout=self.slave, stderr=self.slave, env=child_env,
            start_new_session=True,
        )

        atexit.register(self.abort)

    def abort(self):
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait()

    def screen_text(self):
        # Reconstruct cells, because Ratatui can skip unchanged/default spaces
        # and emit cursor movements in the middle of a visible phrase.
        return "\n".join("".join(self.screen.get((row, col), " ")
                                  for col in range(self.size[0]))
                         for row in range(self.size[1]))

    def record_screen(self, chunk):
        for char in self.decoder.decode(chunk):
            if self.escape:
                self.escape += char
                if self.escape.startswith("\x1b]"):
                    if char == "\x07" or self.escape.endswith("\x1b\\"):
                        self.escape = ""
                    continue
                if len(self.escape) == 2 and char == "[":
                    continue
                if self.escape.startswith("\x1b["):
                    if not ("@" <= char <= "~"):
                        continue
                    args = self.escape[2:-1]
                    row, col = self.cursor
                    if char in "Hf":
                        coords = [int(n or 1) for n in args.split(";")]
                        row, col = (coords + [1])[:2]
                        self.cursor = (row - 1, col - 1)
                    elif char == "J" and args in ["2", "3"]:
                        self.screen.clear()
                    elif char == "K":
                        for x in range(col, self.size[0]):
                            self.screen.pop((row, x), None)
                self.escape = ""
            elif char == "\x1b":
                self.escape = char
            elif char == "\r":
                self.cursor = (self.cursor[0], 0)
            elif char == "\n":
                self.cursor = (self.cursor[0] + 1, self.cursor[1])
            elif char >= " " and not unicodedata.combining(char):
                row, col = self.cursor
                self.screen[(row, col)] = char
                width = 2 if unicodedata.east_asian_width(char) in "WF" else 1
                for offset in range(1, width):
                    self.screen[(row, col + offset)] = ""
                self.cursor = (row, col + width)

    def resize(self, width, height):
        self.size = (width, height)
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))
        if hasattr(self, "process"):
            os.kill(self.process.pid, signal.SIGWINCH)

    def pump(self, seconds=0.2):
        until = time.monotonic() + seconds
        while time.monotonic() < until:
            ready, _, _ = select.select([self.master], [], [], 0.025)
            if ready:
                try:
                    chunk = os.read(self.master, 65536)
                    if not chunk:
                        break
                    self.output.extend(chunk)
                    self.record_screen(chunk)
                    self.queries.extend(chunk)
                    # Emulate the terminal, independently of the app's socket mock.
                    pattern = rb"\x1b\](10|11|4;[1-6]);\?\x1b\\"
                    matches = list(re.finditer(pattern, self.queries))
                    for match in matches:
                        self.query_count += 1
                        command = match[1].decode()
                        if self.colors is not None and command in self.colors:
                            reply = f"\x1b]{command};rgb:{self.colors[command]}\x1b\\"
                            os.write(self.master, reply.encode())
                    if matches:
                        del self.queries[:matches[-1].end()]
                    self.queries = self.queries[-64:]
                except OSError:
                    break

    def send(self, keys):
        os.write(self.master, keys.encode())
        self.pump()

    def ready(self):
        deadline = time.monotonic() + 5
        while b"GIT GRAPH" not in self.output and time.monotonic() < deadline:
            self.pump(0.1)
        assert b"GIT GRAPH" in self.output, repr(bytes(self.output))

    def finish(self, terminate=False):
        if terminate:
            self.process.send_signal(signal.SIGTERM)
        else:
            self.send("q")
        deadline = time.monotonic() + 4
        while self.process.poll() is None and time.monotonic() < deadline:
            self.pump(0.1)
        try:
            assert self.process.poll() == 0, self.output.decode(errors="replace")[-2000:]
            self.pump(0.1)
            restored = termios.tcgetattr(self.slave)
            assert restored == self.original, "Terminal attributes were not restored"
            assert b"\x1b[?1049l" in self.output, "Alternate screen not restored"
            assert b"\x1b[?1006l" in self.output, "Mouse capture not disabled"
        finally:
            if self.process.poll() is None:
                self.process.kill()
                self.process.wait()
            os.close(self.master)
            os.close(self.slave)


class MockHerdr:
    def __init__(self, path, fail=False):
        self.path = path
        self.fail = fail
        self.frames = []
        self.requests = []
        self.errors = []
        self.closed = 0
        self.listener = socket.socket(socket.AF_UNIX)
        self.listener.bind(str(path))
        self.listener.listen()
        self.listener.settimeout(0.2)
        self.stop = False
        self.thread = threading.Thread(target=self.run, daemon=True)
        self.thread.start()

    def run(self):
        while not self.stop:
            try:
                conn, _ = self.listener.accept()
            except socket.timeout:
                continue
            except OSError:
                return
            threading.Thread(target=self.handle, args=(conn,), daemon=True).start()

    def handle(self, conn):
        try:
            with conn, conn.makefile("rwb", buffering=0) as stream:
                request = json.loads(stream.readline())
                self.requests.append(request)
                assert request["params"]["pane_id"] == "w-test:p-test"
                if self.fail:
                    stream.write(json.dumps({"id": request["id"], "error": {"code": "feature_disabled"}}).encode() + b"\n")
                    return
                method = request["method"]
                if method == "pane.graphics.info":
                    result = {"type": "pane_graphics_info", "cell_width_px": 10, "cell_height_px": 20, "pane_visible": True}
                else:
                    assert method == "pane.graphics.stream", method
                    assert request["params"]["layer_id"] == "git-graph"
                    result = {"type": "ok"}
                stream.write(json.dumps({"id": request["id"], "result": result}).encode() + b"\n")
                if method == "pane.graphics.info":
                    return
                while line := stream.readline():
                    header = json.loads(line)
                    data = bytearray()
                    while len(data) < header["data_length"]:
                        part = stream.read(header["data_length"] - len(data))
                        assert part, "Truncated PNG frame"
                        data.extend(part)
                    assert data[:8] == b"\x89PNG\r\n\x1a\n"
                    assert struct.unpack(">II", data[16:24]) == (header["image_width"], header["image_height"])
                    assert header["image_width"] == header["placement"]["grid_cols"] * 10
                    assert header["image_height"] == header["placement"]["grid_rows"] * 20
                    self.frames.append((header, bytes(data)))
                self.closed += 1
        except Exception as error:
            self.errors.append(repr(error))

    def close(self):
        self.stop = True
        self.listener.close()
        self.thread.join(timeout=1)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("binary", type=Path)
    parser.add_argument("--work", type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve()
    args.work.mkdir(parents=True, exist_ok=True)
    terminal = Terminal(binary, ["--renderer", "text"])
    terminal.ready()
    for keys in ["jj", "G", "g", "/한글\r", "n", "\x1b", "\t", "jj", "d", "d", "?", "\x1b"]:
        terminal.send(keys)
    terminal.resize(64, 18)
    terminal.pump()
    terminal.resize(140, 44)
    terminal.pump()
    terminal.finish()
    print("PASS: PTY navigation, Unicode search, help, resize, terminal restoration")

    terminal = Terminal(binary, ["--renderer", "text"])
    terminal.ready()
    terminal.finish(terminate=True)
    print("PASS: SIGTERM restores terminal and mouse state")

    terminal = Terminal(binary, ["--sidebar", "--renderer", "text"], size=(40, 24))
    terminal.ready()
    for keys in ["jj", "\t", "\x1b[Z", "d", "\r", "/한글\r", "n", "\x1b", "?", "\x1b", "r"]:
        terminal.send(keys)
    # Click and scroll within the compact history (SGR mouse protocol).
    terminal.send("\x1b[<0;12;6M\x1b[<0;12;6m\x1b[<65;12;6M")
    for size in [(24, 8), (20, 8), (48, 30), (140, 44)]:
        terminal.resize(*size)
        terminal.pump()
    terminal.finish()
    assert b"COMMIT INSPECTOR" not in terminal.output
    assert b"BRANCHES" not in terminal.output
    print("PASS: narrow sidebar, search, graph-only focus, mouse, resize, cleanup")

    for colors, delay in [(DARK_COLORS, 0.01), (DARK_COLORS, 0.1), (LIGHT_COLORS, 0.1)]:
        terminal = Terminal(binary, ["--renderer", "text"], colors=colors)
        terminal.ready()
        terminal.pump(0.3)
        assert terminal.query_count == 8, terminal.query_count
        # Cyan is the first graph lane and is copied verbatim from OSC 4.
        assert b"38;2;51;187;170" in terminal.output
        assert b"48;2;12;17;24" not in terminal.output, "Fixed background leaked into auto theme"
        terminal.send("/themeprobe")
        # Slow fragments must preserve the search itself, not just hide "rgb:".
        # A leaked 'r' can trigger a query and '/' can reopen search, so checking
        # only query_count or the raw output can miss a broken parser.
        # At 100 ms per character, one reply also exceeds the 700 ms idle
        # timeout in total; continued input must keep that reply alive.
        for fragment in ["\x1b", "]11;rgb:", *colors["11"], "\x07",
                         "\x1b]4;6;rgb:3333/bbbb/aaaa", "\x1b", "\\"]:
            os.write(terminal.master, fragment.encode())
            terminal.pump(delay)
            assert "/ themeprobe▏" in terminal.screen_text(), terminal.screen_text()
            assert terminal.query_count == 8, terminal.query_count
        terminal.send("\r")
        assert "/ themeprobe ·" in terminal.screen_text(), terminal.screen_text()
        terminal.send("r")
        assert terminal.query_count == 16
        assert "/ themeprobe ·" in terminal.screen_text(), terminal.screen_text()
        terminal.finish()
        assert b"rgb:" not in terminal.output, "Color reply leaked into search"
    print("PASS: light/dark OSC colors, inherited background, late/fragmented replies, theme refresh")

    terminal = Terminal(binary, ["--renderer", "curves"])
    terminal.ready()
    terminal.pump(0.9)
    assert "terminal palette unavailable" in terminal.screen_text()
    terminal.send("j")
    terminal.finish()
    print("PASS: unanswered queries keep UI responsive with native colors and text fallback")

    for mode in ["terminal", "classic"]:
        terminal = Terminal(binary, ["--theme", mode, "--renderer", "text"])
        terminal.ready()
        terminal.finish()
        assert terminal.query_count == 0
        assert (b"48;2;12;17;24" in terminal.output) == (mode == "classic")
    print("PASS: query-free terminal mode and classic theme opt-out")

    with tempfile.TemporaryDirectory(prefix="live-", dir=args.work) as temporary:
        repo = Path(temporary)
        def git(*arguments):
            subprocess.run(["git", "-C", str(repo), "-c", "user.name=Test",
                            "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false",
                            *arguments], check=True, capture_output=True)
        git("init", "-b", "main")
        tracked = repo / "tracked.txt"
        tracked.write_text("initial\n")
        git("add", ".")
        git("commit", "-m", "initial")
        terminal = Terminal(binary, ["--repo", str(repo), "--renderer", "text", "--refresh-interval", "1"], demo=False)
        terminal.ready()

        def wait_for(text):
            deadline = time.monotonic() + 6
            while text.decode() not in terminal.screen_text() and time.monotonic() < deadline:
                terminal.pump(0.1)
            assert text.decode() in terminal.screen_text(), terminal.screen_text()

        tracked.write_text("AAAAAAAAAAAA\n")
        wait_for(b"Uncommitted changes")
        terminal.send("g")
        wait_for(b"Files (index / working tree):")
        terminal.send("\rG")
        wait_for(b"AAAAAAAAAAAA")
        terminal.output.clear()
        tracked.write_text("ZZZZZZZZZZZZ\n")
        wait_for(b"ZZZZZZZZZZZZ")
        terminal.output.clear()
        git("add", ".")
        git("commit", "-m", "live commit")
        wait_for(b"live commit")
        terminal.finish()
        print("PASS: live PTY file edits, mutable diff refresh and external commit without r")

        terminal = Terminal(binary, ["--repo", str(repo), "--sidebar", "--renderer", "text",
                                     "--refresh-interval", "1", "--no-auto-refresh"], demo=False)
        terminal.ready()
        tracked.write_text("manual only\n")
        terminal.pump(1.8)
        assert "Uncommitted changes" not in terminal.screen_text()
        terminal.send("r")
        wait_for(b"Uncommitted changes")
        terminal.finish()
        print("PASS: auto-refresh opt-out and manual sidebar reload")

    with tempfile.TemporaryDirectory(prefix="rpc-", dir=args.work) as temporary:
        # UNIX socket paths have a small platform-specific maximum length.
        mock = MockHerdr(Path(temporary) / "s")
        terminal = Terminal(binary, ["--renderer", "curves"], {"HERDR_SOCKET_PATH": str(mock.path), "HERDR_PANE_ID": "w-test:p-test"}, colors=DARK_COLORS)
        terminal.ready()
        terminal.pump(0.2)
        previous_frame = mock.frames[-1][1]
        terminal.colors = dict(LIGHT_COLORS, **{"4;6": "2222/8888/9999"})
        terminal.send("r")
        terminal.pump(0.3)
        assert mock.frames[-1][1] != previous_frame, "Theme change must invalidate cached curves"
        terminal.send("j")
        terminal.send("?")
        terminal.send("\x1b")
        terminal.resize(110, 32)
        terminal.pump(0.3)
        terminal.finish()
        mock.close()
        assert not mock.errors, mock.errors
        assert len(mock.frames) >= 3, len(mock.frames)
        assert mock.closed >= 2, "Help/exit should close owned graphics layers"
        (args.work / "stream-frame.png").write_bytes(mock.frames[-1][1])
        print(f"PASS: Herdr graphics negotiation, {len(mock.frames)} PNG frames, resize, layer cleanup")

    with tempfile.TemporaryDirectory(prefix="rpc-", dir=args.work) as temporary:
        mock = MockHerdr(Path(temporary) / "s")
        terminal = Terminal(binary, ["--sidebar", "--renderer", "curves"],
                            {"HERDR_SOCKET_PATH": str(mock.path), "HERDR_PANE_ID": "w-test:p-test"}, size=(40, 24), colors=LIGHT_COLORS)
        terminal.ready()
        terminal.pump(0.2)
        terminal.send("jl")
        terminal.send("?")
        terminal.send("\x1b")
        terminal.resize(24, 8)
        terminal.pump(0.3)
        terminal.finish()
        mock.close()
        assert not mock.errors, mock.errors
        assert len(mock.frames) >= 3, len(mock.frames)
        assert mock.closed >= 2, "Sidebar help/exit should close graphics layers"
        assert mock.frames[-1][0]["placement"]["grid_cols"] <= 8
        print("PASS: sidebar curves, narrow viewport, panning, help/exit layer cleanup")

    with tempfile.TemporaryDirectory(prefix="rpc-", dir=args.work) as temporary:
        mock = MockHerdr(Path(temporary) / "s", fail=True)
        terminal = Terminal(binary, ["--renderer", "curves"], {"HERDR_SOCKET_PATH": str(mock.path), "HERDR_PANE_ID": "w-test:p-test"}, colors=DARK_COLORS)
        terminal.ready()
        terminal.pump(0.3)
        assert "Text fallback" in terminal.screen_text()
        terminal.finish()
        mock.close()
        assert not mock.frames
        assert not mock.errors, mock.errors
        print("PASS: disabled graphics falls back to text")


if __name__ == "__main__":
    main()
