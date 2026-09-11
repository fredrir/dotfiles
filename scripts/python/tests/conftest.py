import os
import subprocess
import sys
from pathlib import Path

import pytest
from native import rust_binary

BIN = os.path.dirname(sys.executable)
ROOT = Path(__file__).resolve().parents[3]


@pytest.fixture
def tool():
    def invoke(name, *args, env=None, cwd=None, input_text=None):
        environment = dict(os.environ)
        executable = os.path.join(BIN, name)
        if name == "dotfile":
            executable = str(rust_binary("dotfile-cli", "dotfile"))
        if env:
            environment.update(env)
        return subprocess.run(
            [executable, *args],
            capture_output=True,
            text=True,
            env=environment,
            cwd=cwd,
            input=input_text,
            check=False,
        )

    return invoke
