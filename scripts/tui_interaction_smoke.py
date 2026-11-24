#!/usr/bin/env python3
"""Exercise a release TUI in a POSIX PTY against an already-running daemon.

Uses an isolated draft directory. Never sends a prompt or confirms cancellation,
approval, denial, completion or handoff. Startup registers the current workspace
and probes provider diagnostics through the normal TUI initialization path.
"""

import argparse
import fcntl
import json
import os
import pty
import re
import select
import struct
import subprocess
import tempfile
import termios
import time
import tomllib
import unicodedata
import urllib.request
from pathlib import Path


class Client:
    def __init__(self, binary, draft_directory):
        self.master, self.slave = pty.openpty()
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", 38, 112, 0, 0))
        self.original = termios.tcgetattr(self.slave)
        self.output = bytearray()
        env = dict(os.environ, TERM="xterm-256color", BAIZE_DATA_DIR=str(draft_directory))
        self.process = subprocess.Popen(
            [str(binary), "tui"], stdin=self.slave, stdout=self.slave, stderr=self.slave, env=env
        )
        try:
            self.drain(1.5)
            assert "BAIZE" in self.screen(), "TUI did not start"
        except BaseException:
            self.close()
            raise

    def drain(self, seconds=0.3):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            ready, _, _ = select.select([self.master], [], [], min(0.05, max(0, deadline - time.monotonic())))
            if ready:
                try:
                    self.output.extend(os.read(self.master, 65536))
                except OSError:
                    return

    def send(self, data, seconds=0.3):
        os.write(self.master, data)
        self.drain(seconds)

    def screen(self):
        rows = [[" "] * 112 for _ in range(38)]
        y = x = 0
        tokens = re.findall(r"\x1b\[[0-?]*[ -/]*[@-~]|[^\x1b]+", self.output.decode("utf-8", errors="replace"))
        for token in tokens:
            if token.startswith("\x1b["):
                code, args = token[-1], token[2:-1]
                if args.startswith("?"):
                    continue
                values = [int(item) if item else 0 for item in args.split(";")]
                if code in "Hf":
                    y = max(0, min(37, (values[0] or 1) - 1))
                    x = max(0, min(111, ((values[1] if len(values) > 1 else 1) or 1) - 1))
                elif code == "J" and values[0] == 2:
                    rows = [[" "] * 112 for _ in range(38)]
                continue
            for char in token:
                if char == "\r":
                    x = 0
                elif char == "\n":
                    y = min(37, y + 1)
                elif ord(char) >= 32 and x < 112:
                    if unicodedata.combining(char):
                        if x:
                            rows[y][x - 1] += char
                        continue
                    width = 2 if unicodedata.east_asian_width(char) in "WF" else 1
                    rows[y][x] = char
                    if width == 2 and x + 1 < 112:
                        rows[y][x + 1] = ""
                    x += width
        return "\n".join("".join(row) for row in rows)

    def close(self):
        if self.process.poll() is None:
            self.process.terminate()
            self.process.wait(timeout=3)
        os.close(self.master)
        os.close(self.slave)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/baize"))
    parser.add_argument("--output-dir", type=Path, default=Path("target/audit/tui-interactions/pty"))
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    config = tomllib.loads(subprocess.check_output([binary, "config", "show"], text=True))
    daemon = config["daemon"]
    host = "127.0.0.1" if daemon["host"] == "0.0.0.0" else daemon["host"]
    assert host == "127.0.0.1", "This smoke test requires a loopback daemon"
    # Refuse to auto-launch a daemon using the temporary client data directory.
    with urllib.request.urlopen(f"http://{host}:{daemon['port']}/health", timeout=2) as response:
        assert response.status == 200
    args.output_dir.mkdir(parents=True, exist_ok=True)
    draft = "PTY recovery draft 中文\n    indented line"
    with tempfile.TemporaryDirectory(prefix="baize-tui-smoke-") as directory:
        client = Client(binary, directory)
        try:
            client.send(b"\x1b[200~" + draft.encode() + b"\x1b[201~")
            document = json.loads((Path(directory) / "tui-drafts.json").read_text())
            stored = document["workspaces"][str(Path.cwd())]["drafts"][""]
            assert stored[0] == draft, "Pasted draft was not checkpointed"
            client.process.kill()  # Simulate abrupt client failure, without an exit checkpoint.
            client.process.wait(timeout=3)
            (args.output_dir / "before-crash.ansi").write_bytes(client.output)
        finally:
            client.close()

        client = Client(binary, directory)
        try:
            visible = client.screen()
            assert "PTY recovery draft 中文" in visible and "indented line" in visible
            client.send(b"\x1b[Z")  # Shift-Tab: composer -> inspector.
            assert "Session" in client.screen()
            client.send(b"\x1b")
            client.send(b"\x04")  # Ctrl-D must review before making any decision.
            assert "Permissions" in client.screen()
            client.send(b"\x1b")
            client.send(b"\x0c", 1)  # Ctrl-L: read recorded history only.
            client.send(b"\x18")  # Ctrl-X: show cancel dialog, never confirm it.
            cancel_dialog_observed = "Cancel Baize session?" in client.screen()
            if cancel_dialog_observed:
                client.send(b"\x18\x18")
                assert client.process.poll() is None
                assert "Enter cancel" in client.screen()
            else:
                assert "Start or open a session first" in client.screen()
            client.send(b"\x1b")
            client.send(b"\x11")
            assert "Leave Baize?" in client.screen()
            assert "Drafts are saved locally" in client.screen()
            client.send(b"\x03\x03")
            assert client.process.poll() is None and "Enter quit" in client.screen()
            client.send(b"\x1b")
            assert client.process.poll() is None
            client.send(b"\x11")
            assert "Enter quit" in client.screen(), "Refusing Enter outside the quit dialog"
            client.send(b"\r")
            assert client.process.wait(timeout=3) == 0
            assert termios.tcgetattr(client.slave) == client.original
            assert b"\x1b[?1049l" in client.output
            (args.output_dir / "after-restart.ansi").write_bytes(client.output)
            report = {"passed": True, "checks": ["Unicode multiline paste checkpoint", "SIGKILL and restart recovery", "reverse focus", "permission review", "cancel review without confirmation", "saved-draft quit dialog", "repeated exit shortcut protection", "Esc resumes", "terminal mode restoration"], "cancel_dialog_observed": cancel_dialog_observed, "provider_prompts_submitted": 0}
        finally:
            client.close()

        # V1 did not distinguish unsent drafts from possibly executed submissions.
        marker = "LEGACY_REVIEW_INPUT"
        cache = Path(directory) / "tui-drafts.json"
        cache.write_text(json.dumps({"version": 1, "workspaces": {str(Path.cwd()): {"": [marker, 3, []]}}}))
        client = Client(binary, directory)
        try:
            assert marker not in client.screen(), "Legacy input returned as an unsent prompt"
            assert "Ctrl-K" in client.screen()
            client.send(b"\x0b")
            assert "Commands" in client.screen(), "Refusing Enter outside the command palette"
            client.send(b"recover")
            assert "Recover last submitted prompt" in client.screen()
            client.send(b"\r")  # Explicit recovery, never provider submission.
            assert marker in client.screen()
            migrated = json.loads(cache.read_text())
            assert migrated["version"] == 2
            assert migrated["workspaces"][str(Path.cwd())]["drafts"][""][0] == marker
            assert not migrated["workspaces"][str(Path.cwd())]["submissions"]
            client.send(b"\x11")
            assert "Enter quit" in client.screen()
            client.send(b"\r")
            assert client.process.wait(timeout=3) == 0
            assert termios.tcgetattr(client.slave) == client.original
            (args.output_dir / "legacy-review.ansi").write_bytes(client.output)
            report["checks"].append("explicit legacy recovery without automatic composer fill")
            (args.output_dir / "result.json").write_text(json.dumps(report, indent=2) + "\n")
            print(json.dumps(report))
        finally:
            client.close()


if __name__ == "__main__":
    main()
