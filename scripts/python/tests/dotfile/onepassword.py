"""A fake 1Password CLI for the dotfile tests, shared with the Rust suites.

The script lives beside the Rust tests; this module puts it first on PATH with a
temporary item store so no test can reach the real 1Password or key files.
"""

import json
import os
import re
import shutil
from pathlib import Path

FAKE_OP = Path(__file__).resolve().parents[4] / "scripts/rust/crates/dotfile/tests/support/fake-op"
HOST = "machine"
VAULT = "Test"
ITEM = "MACHINE_SOPS_AGE_KEY_FILE"


def install(directory):
    """Returns the environment that routes op, op-bridge and host detection to fakes."""
    bin_dir = directory / "fake-bin"
    store = directory / "fake-1password"
    bin_dir.mkdir(parents=True, exist_ok=True)
    store.mkdir(exist_ok=True)
    shutil.copy(FAKE_OP, bin_dir / "op")
    (bin_dir / "op").chmod(0o755)
    bridge = bin_dir / "op-bridge"
    bridge.write_text("#!/bin/sh\nexit 0\n")
    bridge.chmod(0o755)
    return {
        "PATH": f"{bin_dir}{os.pathsep}{os.environ['PATH']}",
        "FAKE_OP_STORE": str(store),
        "SYSINFO_HOST": HOST,
    }


def declare_host(root):
    """Names this machine and points it at the fake item; keeps any recipients."""
    (root / "config" / "hosts.dotfile").write_text(f"{HOST} {{\n  hostnames = {HOST}\n}}\n")
    keys = root / "config" / "keys.dotfile"
    current = keys.read_text() if keys.exists() else ""
    keys.write_text(current + f"identities {{\n  {HOST} = op://{VAULT}/{ITEM}\n}}\n")


def forget_identity(root):
    keys = root / "config" / "keys.dotfile"
    keys.write_text(re.sub(r"\n*identities \{[^}]*\}\n", "\n", keys.read_text()).lstrip("\n"))


def field(env, name):
    item = Path(env["FAKE_OP_STORE"]) / VAULT / f"{ITEM}.json"
    for entry in json.loads(item.read_text())["fields"]:
        if entry["id"] == name:
            return entry["value"]
    return None


def identity_file(env, destination):
    destination.write_text(field(env, "credential") + "\n")
    return destination
