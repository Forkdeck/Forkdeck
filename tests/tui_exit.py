#!/usr/bin/env python3
"""PTY smoke test: python3 tests/tui_exit.py target/debug/forkdeck (Linux)."""
import json
import os
import pathlib
import pty
import select
import sqlite3
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
import fcntl

binary = str(pathlib.Path(sys.argv[1]).resolve())


def database(project):
    return sqlite3.connect(project / "state/forkdeck/forkdeck.db")


def saved(project):
    """What the input line held for this working directory, as the history database keeps it."""
    try:
        with database(project) as db:
            row = db.execute("SELECT state FROM composer WHERE workdir = ?", (str(project.resolve()),)).fetchone()
    except sqlite3.Error:
        return {"draft": None}
    return json.loads(row[0]) if row else {"draft": None}


def queued(project):
    """Messages the first task had accepted and not taken yet."""
    with database(project) as db:
        row = db.execute("SELECT inbox FROM jobs WHERE source != 'null' ORDER BY id LIMIT 1").fetchone()
    return json.loads(row[0]) if row and row[0] else []
with tempfile.TemporaryDirectory(prefix="forkdeck-exit-") as temporary:
    root = pathlib.Path(temporary)
    agent = root / "agent.py"
    agent.write_text('''#!/usr/bin/python3
import json,pathlib,sys,time
if '--version' in sys.argv:
 print('stand-in');sys.exit(0)
prompt=sys.stdin.read()
if 'slow task' in prompt:
 pathlib.Path('agent-pid').write_text(str(__import__('os').getpid()))
 while True:time.sleep(1)
print(json.dumps({'type':'result','is_error':False,'result':'Choose an approach:\\n1. Small\\n2. Large'}))
''')
    agent.chmod(0o755)
    config = root / "config.toml"
    config.write_text(f'''[router]
kind="rules"
[agents.claude]
cmd="{agent}"
[agents.codex]
cmd="{root}/missing"
[agents.copilot]
cmd="{root}/missing"
[automation]
enabled=false
''')
    for mode in ("draft", "issue-draft", "picker", "busy", "signal"):
        project = root / mode
        project.mkdir()
        master, slave = pty.openpty()
        original = termios.tcgetattr(slave)
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 140, 0, 0))

        def terminal_session():
            os.setsid()
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)

        process = subprocess.Popen(
            [binary, "--config", str(config)], cwd=project,
            env={**os.environ, "TERM": "xterm-256color", "XDG_STATE_HOME": str(project / "state")},
            stdin=slave, stdout=slave, stderr=slave, preexec_fn=terminal_session,
        )
        output = bytearray()

        def wait(predicate, timeout=10):
            end = time.monotonic() + timeout
            while time.monotonic() < end:
                if predicate():
                    return
                if select.select([master], [], [], .05)[0]:
                    output.extend(os.read(master, 65536))
            raise AssertionError(f"{mode}: timed out; return code {process.poll()}\n{output.decode(errors='replace')[-2000:]}")

        try:
            wait(lambda: b"Quick task" in output)
            if mode == "draft":
                os.write(master, b"unfinished draft")
                time.sleep(.2)
            elif mode == "issue-draft":
                os.write(master, b"\x1b[17~\t\tunfinished issue plan")
                wait(lambda: saved(project)["draft"] == "unfinished issue plan")
            elif mode == "picker":
                os.write(master, b"planning options\r")
                wait(lambda: "Planning options · task".encode() in output)
            else:
                os.write(master, b"slow task\r")
                wait(lambda: (project / "agent-pid").exists())
                os.write(master, b"queued follow-up\r")
                wait(lambda: b"1 queued" in output)
            start = time.monotonic()
            if mode == "signal":
                process.send_signal(signal.SIGINT)
            else:
                os.write(master, b"\x03")
            wait(lambda: process.poll() is not None, timeout=5)
            assert time.monotonic() - start < 5
            restored = termios.tcgetattr(slave)
            mask = termios.ICANON | termios.ECHO | termios.ISIG
            assert restored[3] & mask == original[3] & mask, f"{mode}: terminal was not restored"
            if mode == "draft":
                assert saved(project)["draft"] == "unfinished draft"
            if mode == "issue-draft":
                assert saved(project)["draft"] == "unfinished issue plan"
            if mode in ("busy", "signal"):
                assert queued(project) == ["queued follow-up"], queued(project)
                pid = int((project / "agent-pid").read_text())
                try:
                    os.kill(pid, 0)
                except ProcessLookupError:
                    pass
                else:
                    # A killed child may briefly remain a zombie until reaped by the runtime.
                    assert pathlib.Path(f"/proc/{pid}/stat").read_text().split()[2] == "Z", "agent still running"
            print(f"PASS: {mode} exits promptly, restores the terminal, and saves pending work")
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            os.close(master)
            os.close(slave)
