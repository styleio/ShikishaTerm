#!/usr/bin/env python3
"""
What an AI CLI writes to its terminal, recorded byte for byte on Linux: the
material the terminal-state tests move through (far-keep plan §4.4, §10.1
"the state moved over, with real CLIs").

Each CLI is started in a pseudo-terminal of its own in an empty folder, left
to draw its first screen, typed a few words into WITHOUT sending them (no AI
turn is asked for, nothing is billed), and ended with Ctrl+C. What it wrote
goes to target/tty/<cli>.bin, which stays out of the repository: a CLI's
first screen can carry the account's name.

    python3 tools/debug/record-tui.py [claude codex gemini]

Then, on any machine:

    cargo test -p shikisha-core recorded_ -- --ignored
"""
import os
import pty
import select
import sys
import tempfile
import time
import fcntl
import struct
import termios

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
OUT = os.path.join(ROOT, "target", "tty")
ROWS, COLS = 30, 100


def record(cli):
    here = tempfile.mkdtemp(prefix="sk-tty-")
    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(here)
        os.environ["TERM"] = "xterm-256color"
        os.execvp(cli, [cli])
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
    got = bytearray()

    def drain(seconds):
        end = time.time() + seconds
        while time.time() < end:
            r, _, _ = select.select([fd], [], [], 0.2)
            if fd in r:
                try:
                    chunk = os.read(fd, 65536)
                except OSError:
                    return False
                if not chunk:
                    return False
                got.extend(chunk)
                # Answer the questions a terminal is asked, as one would, so
                # the CLI goes on drawing
                if b"\x1b[6n" in chunk:
                    os.write(fd, b"\x1b[1;1R")
                if b"\x1b[c" in chunk:
                    os.write(fd, b"\x1b[?6c")
        return True

    alive = drain(12)
    if alive:
        for ch in "a few words, not sent":
            os.write(fd, ch.encode())
            drain(0.05)
        drain(2)
        os.write(fd, b"\x15")  # clear the line, still nothing sent
        drain(1)
        os.write(fd, b"\x03")
        drain(1)
        os.write(fd, b"\x03")
        drain(3)
    try:
        os.kill(pid, 9)
    except OSError:
        pass
    os.makedirs(OUT, exist_ok=True)
    path = os.path.join(OUT, cli + ".bin")
    with open(path, "wb") as f:
        f.write(bytes(got))
    print(f"{cli}: {len(got)} bytes -> {path}")


for cli in sys.argv[1:] or ["claude", "codex", "gemini"]:
    record(cli)
