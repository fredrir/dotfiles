import os
import subprocess
import sys
from pathlib import Path

import pytest

BIN = os.path.dirname(sys.executable)
ROOT = Path(__file__).resolve().parents[3]


@pytest.fixture
def tool():
    def invoke(name, *args, env=None, cwd=None, input_text=None):
        environment = dict(os.environ)
        executable = os.path.join(BIN, name)
        if name == "dotfile":
            native = ROOT / "scripts" / "rust" / "target" / "debug" / "dotfile"
            if args[:1] == ("sync",) and not native.is_file():
                pytest.skip("native dotfile binary is not built")
            executable = str(native) if native.is_file() else os.path.join(BIN, "dotfile-py")
            environment["DOTFILE_PYTHON"] = os.path.join(BIN, "dotfile-py")
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
