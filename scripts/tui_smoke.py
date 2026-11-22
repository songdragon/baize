#!/usr/bin/env python3
"""POSIX PTY smoke test against a loopback fixture; never calls providers.

Requires target/debug/baize and an unused configured daemon port.
Reads user configuration without changing it. Python 3.11+ is recommended.
"""

import fcntl
import json
import os
import pty
import select
import struct
import subprocess
import termios
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


def main():
    binary = str(Path("target/debug/baize").resolve())
    config_path = subprocess.check_output([binary, "config", "path"], text=True).strip()
    host, port = "127.0.0.1", 7878
    if Path(config_path).exists():
        import tomllib

        config = tomllib.loads(Path(config_path).read_text())
        host = config.get("daemon", {}).get("host", host)
        port = config.get("daemon", {}).get("port", port)
    if host == "0.0.0.0":
        host = "127.0.0.1"
    assert host == "127.0.0.1", "Smoke fixture expects an IPv4 loopback configuration"
    requests = []
    fixtures = {
        "/health": {"status": "ok"},
        "/providers": {"providers": [{"id": "codex", "enabled": True}]},
        "/providers/diagnose": {"diagnostics": []},
        "/permissions": {"permissions": []},
        "/runtime/status": {
            "runtime": {
                "command_policy": "ask",
                "execution_mode": "ask_before_commands",
                "routing": {},
            }
        },
        "/workspaces": {"workspace": {"id": "ws_smoke", "name": "smoke fixture"}},
        "/sessions": {"sessions": []},
    }

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_GET(self):
            self.respond()

        def do_POST(self):
            self.rfile.read(int(self.headers.get("Content-Length", "0")))
            self.respond()

        def respond(self):
            requests.append((self.command, self.path))
            path = self.path.split("?")[0]
            assert not path.endswith("/prompt"), "Smoke test must never submit a prompt"
            value = fixtures.get(path, {"error": "unexpected smoke request"})
            data = json.dumps(value).encode()
            self.send_response(200)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

    server = ThreadingHTTPServer((host, port), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 150, 0, 0))
    original = termios.tcgetattr(slave)
    process = subprocess.Popen([binary, "tui"], stdin=slave, stdout=slave, stderr=slave)
    output = bytearray()

    def drain(seconds):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            ready, _, _ = select.select(
                [master], [], [], min(0.1, max(0, deadline - time.monotonic()))
            )
            if ready:
                try:
                    output.extend(os.read(master, 65536))
                except OSError:
                    return

    try:
        drain(1.2)
        os.write(master, b"\x03")  # Ctrl-C must never quit directly, including idle.
        drain(0.3)
        assert process.poll() is None, "Ctrl-C exited without confirmation"
        assert b"Leave Baize?" in output, "Idle exit confirmation did not render"
        os.write(master, b"\x03\x03")
        drain(0.2)
        assert process.poll() is None, "Repeated Ctrl-C bypassed exit confirmation"
        os.write(master, b"\x1b")
        drain(0.2)
        os.write(master, b"\x0b")  # Ctrl-K: command palette.
        drain(0.3)
        assert b"Commands" in output, "Command palette did not render"
        os.write(master, b"\x1b")
        drain(0.1)
        os.write(master, b"\x1b[200~unsent\n    draft " + "中文".encode() + b"\x1b[201~")
        drain(0.3)
        os.write(master, b"\x11")  # Ctrl-Q: confirm unsent draft.
        drain(0.3)
        assert b"Leave Baize?" in output, "Exit confirmation did not render"
        os.write(master, b"\x1b")
        drain(0.2)
        os.write(master, b"\x0e")  # Ctrl-N: stash previous draft.
        drain(0.2)
        output.clear()
        os.write(master, b"\x11")
        drain(0.2)
        assert b"Leave Baize?" in output, "Stashed draft was not protected on exit"
        os.write(master, b"\r")
        drain(0.3)
        assert process.wait(timeout=3) == 0, "TUI exited with an error"
        assert termios.tcgetattr(slave) == original, "Terminal modes were not restored"
        assert not any(path.endswith("/prompt") for _, path in requests)
        print(
            json.dumps(
                {
                    "result": "passed",
                    "checks": [
                        "startup",
                        "idle and repeated Ctrl-C exit protection",
                        "command palette",
                        "multiline Unicode paste",
                        "draft exit confirmation",
                        "new-session draft retention",
                        "terminal mode restoration",
                    ],
                    "requests": len(requests),
                    "provider_prompts": 0,
                }
            )
        )
    finally:
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=3)
        server.shutdown()
        server.server_close()
        os.close(master)
        os.close(slave)


if __name__ == "__main__":
    main()
