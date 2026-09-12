#!/usr/bin/env python3
"""PTY and Herdr socket smoke tests. Python standard library only; macOS/Linux."""
import argparse
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import socket
import struct
import subprocess
import tempfile
import termios
import threading
import time


class Terminal:
    def __init__(self, binary, extra=(), env=None, size=(140, 44)):
        self.master, self.slave = pty.openpty()
        self.original = termios.tcgetattr(self.slave)
        self.output = bytearray()
        self.resize(*size)
        child_env = dict(os.environ, TERM="xterm-256color")
        for key in ["HERDR_SOCKET_PATH", "HERDR_PANE_ID"]:
            child_env.pop(key, None)
        child_env.update(env or {})
        self.process = subprocess.Popen(
            [str(binary), "--demo", *extra], stdin=self.slave,
            stdout=self.slave, stderr=self.slave, env=child_env,
            start_new_session=True,
        )

    def resize(self, width, height):
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

    with tempfile.TemporaryDirectory(prefix="rpc-", dir=args.work) as temporary:
        # UNIX socket paths have a small platform-specific maximum length.
        mock = MockHerdr(Path(temporary) / "s")
        terminal = Terminal(binary, ["--renderer", "curves"], {"HERDR_SOCKET_PATH": str(mock.path), "HERDR_PANE_ID": "w-test:p-test"})
        terminal.ready()
        terminal.pump(0.2)
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
                            {"HERDR_SOCKET_PATH": str(mock.path), "HERDR_PANE_ID": "w-test:p-test"}, size=(40, 24))
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
        terminal = Terminal(binary, ["--renderer", "curves"], {"HERDR_SOCKET_PATH": str(mock.path), "HERDR_PANE_ID": "w-test:p-test"})
        terminal.ready()
        terminal.finish()
        mock.close()
        assert b"Text fallback" in terminal.output
        assert not mock.frames
        assert not mock.errors, mock.errors
        print("PASS: disabled graphics falls back to text")


if __name__ == "__main__":
    main()
