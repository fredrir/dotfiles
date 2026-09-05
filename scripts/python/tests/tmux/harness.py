import fcntl
import json
import os
import pty
import select
import shutil
import struct
import subprocess
import tempfile
import termios
import threading
import time
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[4]
SOURCE = ROOT / "shared/tmux"


def wait_for(predicate, timeout=5):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.02)
    raise AssertionError("condition timed out")


def rust_binary(package, name):
    subprocess.run(
        [
            "cargo",
            "build",
            "--locked",
            "--manifest-path",
            str(ROOT / "scripts/rust/Cargo.toml"),
            "-p",
            package,
        ],
        check=True,
        timeout=180,
    )
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "scripts/rust/target")).resolve()
    return target / "debug" / name


@pytest.fixture(scope="session")
def workspace_binary():
    override = os.environ.get("TMUX_WORKSPACE_BINARY")
    if override:
        binary = Path(override).resolve()
        assert binary.is_file(), binary
        return binary
    return rust_binary("tmux-workspace", "tmux-workspace")


@pytest.fixture(scope="session")
def tmux_binary():
    binary = os.environ.get("TMUX_BINARY") or shutil.which("tmux")
    if not binary:
        pytest.fail("tmux 3.7c+ required")
    return str(Path(binary).resolve())


@pytest.fixture
def environment(tmp_path, workspace_binary, tmux_binary):
    home = tmp_path / "home"
    home.mkdir()
    config = home / ".config/tmux"
    shutil.copytree(SOURCE, config, ignore=shutil.ignore_patterns("tests", "__pycache__"))
    (config / "workspace.toml").write_text(
        "[projects]\nroots = []\nzoxide = false\nworktrees = false\n"
    )
    fakebin = tmp_path / "bin"
    fakebin.mkdir()
    (fakebin / "tmux").symlink_to(tmux_binary)
    (fakebin / "tmux-workspace").symlink_to(workspace_binary)
    inventory = tmp_path / "hosts.dotfile"
    inventory.write_text("first {\n hostnames = first\n}\nsecond {\n hostnames = second\n}\n")
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("TMUX", "DOTFILES_TMUX", "FZF_"))
    }
    env.update(
        HOME=str(home),
        XDG_CONFIG_HOME=str(home / ".config"),
        XDG_DATA_HOME=str(home / ".local/share"),
        XDG_STATE_HOME=str(home / ".local/state"),
        DOTFILES_TMUX_CONFIG=str(config),
        DOTFILES_TMUX_BINARY=str(workspace_binary),
        DOTFILES_TMUX_PLUGIN_HOME=str(home / ".local/share/tmux/plugins"),
        DOTFILES_TMUX_OFFLINE="1",
        DOTFILES_HOSTS_FILE=str(inventory),
        TMUX_BINARY=tmux_binary,
        TERM="xterm-256color",
        PATH=str(fakebin) + os.pathsep + os.environ["PATH"],
    )
    return env


@pytest.fixture
def invoke(workspace_binary, environment):
    def run(*args, check=True, env=None, cwd=None, input_text=None):
        result = subprocess.run(
            [str(workspace_binary), *map(str, args)],
            env=environment | (env or {}),
            cwd=cwd or environment["HOME"],
            capture_output=True,
            text=True,
            input=input_text,
            timeout=20,
            check=False,
        )
        if check:
            assert result.returncode == 0, (args, result.stdout, result.stderr)
        return result

    return run


class Terminal:
    def __init__(self, server, session, managed=False):
        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 160, 0, 0))
        tty_name = os.ttyname(slave)
        argv = (
            [str(server.binary), "--socket", server.socket, "enter", "--session", session]
            if managed
            else server.base + ["attach-session", "-t", session]
        )
        self.process = subprocess.Popen(
            argv,
            stdin=slave,
            stdout=slave,
            stderr=slave,
            env=server.env,
            start_new_session=True,
        )
        os.close(slave)
        self.buffer = bytearray()
        self.lock = threading.Lock()
        self.stopped = threading.Event()
        self.reader = threading.Thread(target=self.drain, daemon=True)
        self.reader.start()
        try:
            self.name = wait_for(
                lambda: next(
                    (
                        row.split("\t")[0]
                        for row in server.tm(
                            "list-clients", "-F", "#{client_name}\t#{client_tty}"
                        ).splitlines()
                        if row.endswith("\t" + tty_name)
                    ),
                    None,
                )
            )
        except BaseException:
            self.close()
            raise

    def drain(self):
        while not self.stopped.is_set():
            try:
                if select.select([self.master], [], [], 0.05)[0]:
                    data = os.read(self.master, 65536)
                    if not data:
                        return
                    with self.lock:
                        self.buffer.extend(data)
            except (OSError, ValueError):
                return

    def press(self, data):
        os.write(self.master, data)

    def output(self):
        with self.lock:
            return bytes(self.buffer)

    def resize(self, rows, columns):
        import signal

        fcntl.ioctl(self.master, termios.TIOCSWINSZ, struct.pack("HHHH", rows, columns, 0, 0))
        os.kill(self.process.pid, signal.SIGWINCH)

    def close(self):
        try:
            self.process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            self.process.terminate()
            try:
                self.process.wait(timeout=1)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=1)
        finally:
            self.stopped.set()
            self.reader.join(timeout=1)
            os.close(self.master)


class Server:
    def __init__(self, env, binary, tmux):
        self.env = env
        self.binary = binary
        self.directory = tempfile.TemporaryDirectory(prefix="tw-", dir="/tmp")
        self.socket = str(Path(self.directory.name) / "socket")
        self.base = [tmux, "-S", self.socket]
        self.clients = []
        try:
            self.start()
        except BaseException:
            self.close()
            raise

    def start(self):
        self.tm(
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            "origin",
            "-c",
            self.env["HOME"],
            "-x",
            "160",
            "-y",
            "40",
            "/bin/sh",
        )
        self.tm("set-option", "-g", "default-shell", "/bin/sh")
        self.tm("set-option", "-g", "default-command", "/bin/sh")
        self.tm("set-option", "-g", "@workspace_config", self.env["DOTFILES_TMUX_CONFIG"])
        self.pane = self.tm("display-message", "-p", "-t", "origin:", "#{pane_id}")

    def tm(self, *args, check=True):
        result = subprocess.run(
            self.base + list(args),
            capture_output=True,
            text=True,
            env=self.env,
            timeout=10,
            check=False,
        )
        if check:
            assert result.returncode == 0 and not result.stderr, (
                args,
                result.stdout,
                result.stderr,
            )
        return result.stdout.rstrip("\n")

    def run(self, *args, check=True, pane=None, client=None, env=None, timeout=20):
        if env and "TMUX_PICK_MATCH" in env:
            self.tm("set-environment", "-g", "TMUX_PICK_MATCH", env["TMUX_PICK_MATCH"])
        argv = [str(self.binary), "--socket", self.socket, "--pane", pane or self.pane]
        if client:
            argv += ["--client", client]
        result = subprocess.run(
            argv + list(map(str, args)),
            capture_output=True,
            text=True,
            env=self.env | (env or {}),
            cwd=self.env["HOME"],
            timeout=timeout,
            check=False,
        )
        if check:
            assert result.returncode == 0, (args, result.stdout, result.stderr)
        return result

    def attach(self, session="origin"):
        client = Terminal(self, session)
        self.clients.append(client)
        return client

    def load(self):
        self.tm("source-file", str(Path(self.env["DOTFILES_TMUX_CONFIG"]) / ".tmux.conf"))

    def fmt(self, expression, pane=None, client=None):
        args = ["display-message", "-p"]
        if pane or not client:
            args += ["-t", pane or self.pane]
        if client:
            args += ["-c", client]
        return self.tm(*args, expression)

    def capture(self, pane=None):
        return self.tm("capture-pane", "-p", "-S", "-100", "-t", pane or self.pane)

    def panes(self):
        return json.loads(self.run("panes").stdout)

    def stop(self):
        pid = self.tm("display-message", "-p", "#{pid}", check=False)
        self.tm("kill-server", check=False)
        for client in self.clients:
            client.close()
        self.clients.clear()
        if pid.isdigit():

            def stopped():
                try:
                    os.kill(int(pid), 0)
                except ProcessLookupError:
                    return True
                return False

            wait_for(stopped)

    def close(self):
        self.stop()
        self.directory.cleanup()


@pytest.fixture
def server(environment, workspace_binary, tmux_binary):
    instance = Server(environment, workspace_binary, tmux_binary)
    try:
        yield instance
    finally:
        instance.close()


@pytest.fixture
def picker(environment):
    import sys

    target = Path(environment["PATH"].split(os.pathsep)[0]) / "fzf"
    target.write_text(
        f"#!{sys.executable}\n"
        "import os,sys\n"
        "rows=sys.stdin.read().splitlines()\n"
        "query=os.environ.get('TMUX_PICK_MATCH','')\n"
        "if query == '__cancel__': sys.exit(130)\n"
        "match=next((r for r in rows if query in r),None)\n"
        "if match is None: sys.exit(1)\n"
        "print(match)\n"
    )
    target.chmod(0o700)
    return target
