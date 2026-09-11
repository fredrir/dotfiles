import fcntl
import json
import os
import pty
import select
import signal
import struct
import termios
import time
from pathlib import Path

import pytest
from native import ROOT, rust_binary


@pytest.fixture
def history(tmp_path):
    source = ROOT / "scripts/rust/crates/sysinfo/tests/fixtures/bench/archie-schema1.json"
    run = json.loads(source.read_text())
    directory = tmp_path / "benchmarks" / run["host"]
    directory.mkdir(parents=True)
    (directory / f"{run['run_id']}.json").write_text(source.read_text())
    return dict(
        os.environ,
        HOME=str(tmp_path),
        DOTFILE_ROOT=str(tmp_path),
        SYSINFO_BENCHMARKS=str(directory.parent),
        SYSINFO_HOST=run["host"],
        TERM="xterm-256color",
        NO_COLOR="1",
        PATH="/usr/bin:/bin",
    )


class Terminal:
    def __init__(self, environment):
        binary = rust_binary("workstation-sysinfo", "sysinfo")
        self.pid, self.master = pty.fork()
        if self.pid == 0:
            os.chdir(environment["HOME"])
            os.execve(binary, [str(binary), "bench", "show"], environment)
        self.output = bytearray()
        self.status = None
        self.resize(24, 100)

    def resize(self, rows, columns):
        fcntl.ioctl(self.master, termios.TIOCSWINSZ, struct.pack("HHHH", rows, columns, 0, 0))

    def expect(self, text):
        deadline = time.monotonic() + 10
        while text.encode() not in self.output:
            assert time.monotonic() < deadline, self.output.decode(errors="replace")
            if select.select([self.master], [], [], 0.05)[0]:
                try:
                    chunk = os.read(self.master, 65536)
                except OSError:
                    chunk = b""
                assert chunk, self.output.decode(errors="replace")
                self.output.extend(chunk)

    def finish(self):
        deadline = time.monotonic() + 5
        while self.status is None:
            if select.select([self.master], [], [], 0.02)[0]:
                try:
                    self.output.extend(os.read(self.master, 65536))
                except OSError:
                    pass
            pid, status = os.waitpid(self.pid, os.WNOHANG)
            if pid:
                self.status = status
                break
            assert time.monotonic() < deadline, "menu did not exit"
            time.sleep(0.02)
        assert os.waitstatus_to_exitcode(self.status) == 0
        flags = termios.tcgetattr(self.master)[3]
        assert flags & termios.ICANON
        assert flags & termios.ECHO

    def close(self):
        os.close(self.master)
        if self.status is None:
            os.kill(self.pid, signal.SIGKILL)
            os.waitpid(self.pid, 0)


@pytest.mark.parametrize("cancel", [b"q", b"\x03"])
def test_native_history_menu_restores_terminal_after_resize_and_cancel(history, cancel):
    terminal = Terminal(history)
    try:
        terminal.expect("sysinfo bench")
        terminal.expect("archie")
        terminal.resize(10, 22)
        os.write(terminal.master, b"j")
        os.write(terminal.master, cancel)
        terminal.finish()
        assert not (Path(history["SYSINFO_BENCHMARKS"]) / "baselines.json").exists()
    finally:
        terminal.close()
