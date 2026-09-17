import os
import subprocess
import time
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[4]
SCRIPT = ROOT / "scripts/shell/home-lan-connect"
HOST = "archie.local"
PAIR = "192.168.1.178 192.168.1.162"

STUBS = {
    "uname": "echo Darwin",
    "dscacheutil": """
echo lookup >> "$STUB_LOG"
[ -f "$STUB_DOWN" ] || printf 'name: archie.local\\nip_address: 192.168.1.162\\n'
""",
    "route": "printf 'interface: en0\\n'",
    "ipconfig": "echo 192.168.1.178",
}


@pytest.fixture
def lan(tmp_path):
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    for name, body in STUBS.items():
        stub = bin_dir / name
        stub.write_text(f"#!/bin/sh\n{body}\n")
        stub.chmod(0o755)

    class Lan:
        state = tmp_path / "state"
        log = tmp_path / "lookups"
        down = tmp_path / "down"

        def run(self, *args):
            env = {
                **os.environ,
                "PATH": f"{bin_dir}:{os.environ['PATH']}",
                "HOME_LAN_CONNECT_STATE": str(self.state),
                "STUB_LOG": str(self.log),
                "STUB_DOWN": str(self.down),
            }
            return subprocess.run(
                [SCRIPT, *args], capture_output=True, text=True, env=env, check=False
            )

        def lookups(self):
            return len(self.log.read_text().splitlines()) if self.log.exists() else 0

    return Lan()


def test_refresh_stores_the_pair_that_resolve_reuses(lan):
    assert lan.run("--refresh", HOST).stdout.strip() == PAIR
    assert (lan.state / HOST).read_text() == PAIR

    resolved = lan.run("--resolve", HOST)

    assert resolved.returncode == 0
    assert resolved.stdout.strip() == PAIR
    assert lan.lookups() == 1


def test_a_fresh_absent_pair_fails_without_a_lookup(lan):
    lan.down.touch()
    assert lan.run("--refresh", HOST).returncode == 1
    assert (lan.state / HOST).read_text() == ""

    resolved = lan.run("--resolve", HOST)

    assert resolved.returncode == 1
    assert "not on 192.168.1.0/24" in resolved.stderr
    assert lan.lookups() == 1


def test_a_stale_pair_is_looked_up_again(lan):
    lan.run("--refresh", HOST)
    stale = time.time() - 120
    os.utime(lan.state / HOST, (stale, stale))
    lan.down.touch()

    resolved = lan.run("--resolve", HOST)

    assert resolved.returncode == 1
    assert "has no IPv4 address" in resolved.stderr
    assert lan.lookups() == 2


def test_resolve_without_state_looks_up_and_stores_nothing(lan):
    resolved = lan.run("--resolve", HOST)

    assert resolved.stdout.strip() == PAIR
    assert lan.lookups() == 1
    assert not lan.state.exists()


@pytest.mark.parametrize("host", ["../escape", ".hidden", "a/b"])
def test_hosts_that_escape_the_state_directory_are_rejected(lan, host):
    rejected = lan.run("--refresh", host)

    assert rejected.returncode == 1
    assert "invalid host" in rejected.stderr
    assert not lan.state.exists()
