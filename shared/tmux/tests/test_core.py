"""Exercise the native configuration through real tmux terminal clients.

Run with python3 -m unittest discover -s shared/tmux/tests -p test_core.py.
TMUX_BINARY may point to an older tmux build for the compatibility checks.
Every test owns an isolated socket; no existing tmux server is reconfigured.
"""

from __future__ import annotations

import base64
import fcntl
import os
import pty
import re
import select
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
import unittest
import uuid
from pathlib import Path

CONFIG = Path(__file__).resolve().parents[1]
TMUX = os.environ.get("TMUX_BINARY", shutil.which("tmux") or "")
MODULES = ("00-core.conf", "10-keys.conf", "20-copy.conf", "theme.conf", "30-ui.conf")


@unittest.skipUnless(TMUX, "tmux is required")
class CoreTerminalTests(unittest.TestCase):
    def setUp(self):
        self.base = [TMUX, "-L", "dotfiles-core-test-" + uuid.uuid4().hex]
        self.directory = tempfile.TemporaryDirectory(prefix="tmux-core-state-")
        self.addCleanup(self.directory.cleanup)
        state = Path(self.directory.name)
        self.env = dict(
            os.environ,
            TERM="xterm-256color",
            DOTFILES_TMUX_OFFLINE="1",
            DOTFILES_TMUX_PLUGIN_HOME=str(state / "plugins"),
            XDG_DATA_HOME=str(state / "data"),
            XDG_STATE_HOME=str(state / "state"),
        )
        self.env.pop("TMUX", None)
        self.env.pop("TMUX_PANE", None)
        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 36, 160, 0, 0))
        self.client = subprocess.Popen(
            self.base + ["-f", "/dev/null", "new-session", "-s", "core-test", "cat"],
            stdin=slave,
            stdout=slave,
            stderr=slave,
            env=self.env,
            start_new_session=True,
        )
        os.close(slave)
        self.addCleanup(self.cleanup_server)
        self.output = bytearray()
        self.settle()
        self.tm("set", "-g", "@workspace_config", str(CONFIG))
        for module in MODULES:
            self.tm("source-file", str(CONFIG / module))
        self.tm("set", "-g", "default-command", "cat")
        self.tm("set", "-g", "default-shell", "/bin/sh")
        self.client_name = self.tm("list-clients", "-F", "#{client_name}")
        self.first = self.tm("display-message", "-p", "#{pane_id}")

    def cleanup_server(self):
        subprocess.run(
            self.base + ["kill-server"], capture_output=True, env=self.env, check=False
        )
        self.client.wait(timeout=3)
        os.close(self.master)

    def tm(self, *args):
        result = subprocess.run(
            self.base + list(args),
            capture_output=True,
            text=True,
            env=self.env,
            check=False,
        )
        self.assertEqual(result.returncode, 0, (args, result.stdout, result.stderr))
        # source-file can report option errors even when its exit code is zero.
        self.assertFalse(result.stderr, (args, result.stderr))
        return result.stdout.strip()

    def settle(self, duration=0.15):
        deadline = time.monotonic() + duration
        while time.monotonic() < deadline:
            ready, _, _ = select.select(
                [self.master], [], [], max(0, deadline - time.monotonic())
            )
            if ready:
                try:
                    self.output.extend(os.read(self.master, 65536))
                except OSError:
                    break

    def press(self, data):
        os.write(self.master, data)
        # A synthetic terminal does not answer DA/colour requests. tmux extends
        # lone-Escape detection while those startup queries remain outstanding.
        self.settle(0.7 if data.endswith(b"\x1b") else 0.15)

    def client_format(self, expression):
        return self.tm("display-message", "-p", "-c", self.client_name, expression)

    def capture(self, pane=None):
        return self.tm("capture-pane", "-p", "-S", "-100", "-t", pane or self.first)

    def test_reload_and_nonrepeating_navigation(self):
        capabilities = self.tm("show-options", "-s", "terminal-features")
        hooks = self.tm("show-hooks", "-g")
        for _ in range(2):
            self.tm("source-file", str(CONFIG / ".tmux.conf"))
        self.assertEqual(
            capabilities, self.tm("show-options", "-s", "terminal-features")
        )
        self.assertEqual(hooks, self.tm("show-hooks", "-g"))
        keys = self.tm("list-keys", "-T", "prefix")
        for key in "hjkl":
            line = next(
                line
                for line in keys.splitlines()
                if re.search(r"prefix\s+" + key + r"\s", line)
            )
            self.assertNotIn(" -r ", line)
        self.press(b"\x02d")
        self.assertEqual(len(self.tm("list-panes").splitlines()), 2)
        second = self.tm("display-message", "-p", "#{pane_id}")
        self.press(b"\x02hhello\r")
        self.assertIn("hello", self.capture())
        self.assertNotIn("hello", self.capture(second))

    def test_special_key_bytes_survive_real_input_decoder(self):
        reader = (
            "import os,tty;tty.setraw(0);"
            'exec("while True:\\n b=os.read(0,1)\\n if not b: break\\n '
            "os.write(1,b.hex().encode()+b'\\\\r\\\\n')\")"
        )
        self.tm(
            "respawn-pane", "-k", "-t", self.first, sys.executable, "-u", "-c", reader
        )
        self.settle()
        # tmux parses CSI-u before user-keys. Super+s (115;9u) is lossy; the
        # dedicated Yazi sequence deliberately avoids that decoder path.
        payload = b"\x1b[13;2u\x1b[5;30012~\x03\x04\x1bb\x1bf"
        self.press(payload)
        # The reserved wire key translates to the longstanding ZLE sequence.
        # Literal injection bypasses tmux's Super-to-Meta decoder ambiguity.
        expected = payload.replace(b"\x1b[5;30012~", b"\x1b[115;9u")
        self.assertEqual(
            " ".join(self.capture().split()),
            " ".join(f"{byte:02x}" for byte in expected),
        )

    def test_resize_and_nested_modes_are_client_local(self):
        self.press(b"\x02d")
        self.press(b"\x02h")
        self.press(b"\x02R")
        self.assertEqual(self.client_format("#{client_key_table}"), "workspace-resize")
        self.assertIn("RESIZE", self.client_format("#{E:status-right}"))
        self.press(b"hl")
        self.assertEqual(self.client_format("#{client_key_table}"), "workspace-resize")
        self.press(b"\x1b")
        self.assertEqual(self.client_format("#{client_key_table}"), "root")
        self.press(b"\x02B")
        self.assertEqual(self.client_format("#{client_key_table}"), "workspace-nested")
        self.assertIn("INNER", self.client_format("#{E:status-right}"))
        self.press(b"nested text\r")
        self.assertIn("nested text", self.capture())
        self.press(b"\x02\x1b")
        self.assertEqual(self.client_format("#{client_key_table}"), "root")

    def test_clipboard_protocol_reaches_the_attached_terminal(self):
        payload = "workspace OSC52 roundtrip"
        encoded = base64.b64encode(payload.encode())
        self.output.clear()
        self.tm("set-buffer", "-w", "-t", self.client_name, payload)
        self.settle()
        self.assertIn(b"\x1b]52;", self.output)
        self.assertIn(encoded, self.output)
        self.assertEqual(self.tm("show-buffer"), payload)
        version = self.tm("display-message", "-p", "#{version}")
        if tuple(map(int, re.match(r"(\d+)\.(\d+)", version).groups())) < (3, 7):
            return  # Application clipboard reads were added in tmux 3.7.
        reader = (
            "import os,tty;tty.setraw(0);"
            "os.write(1,bytes((27,))+b']52;c;?'+bytes((7,)));"
            "reply=os.read(0,4096);os.write(1,repr(reply).encode());"
            "os.read(0,1)"
        )
        self.output.clear()
        self.tm(
            "respawn-pane", "-k", "-t", self.first, sys.executable, "-u", "-c", reader
        )
        self.settle()
        self.assertIn(b"\x1b]52;", self.output)
        self.press(b"\x1b]52;c;" + encoded + b"\x07")
        self.assertIn(encoded.decode(), self.capture())

    def test_responsive_status_and_operational_indicators(self):
        wide = self.client_format("#{E:status-left}")
        host = self.client_format("#h")
        self.assertIn("core-test", wide)
        self.assertIn(host, wide)
        fcntl.ioctl(self.master, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 60, 0, 0))
        os.kill(self.client.pid, signal.SIGWINCH)
        self.settle()
        narrow = self.client_format("#{E:status-left}")
        self.assertNotIn("core-test", narrow)
        self.assertIn(host, narrow)
        self.press(b"\x02\r")
        self.assertIn("COPY", self.client_format("#{E:status-right}"))
        self.tm("send-keys", "-X", "cancel")
        self.press(b"\x02d")
        self.tm("resize-pane", "-Z")
        self.tm("set-window-option", "synchronize-panes", "on")
        indicator = self.client_format("#{E:status-right}")
        self.assertIn("SYNC", indicator)
        self.assertIn("ZOOM", indicator)
        # Unescaped commas inside tmux conditionals leave broken style tails.
        self.assertNotIn("bold]", re.sub(r"#\[[^\]]*\]", "", indicator))
        self.tm("set-option", "-p", "@workspace-tool", "scratch-view")
        self.assertIn("scratch", self.client_format("#{E:pane-border-format}"))


if __name__ == "__main__":
    unittest.main()
