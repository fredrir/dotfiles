import os
import subprocess
import sys

import pytest

BIN = os.path.dirname(sys.executable)


@pytest.fixture
def tool():
    def invoke(name, *args, env=None, cwd=None, input_text=None):
        environment = dict(os.environ)
        if env:
            environment.update(env)
        return subprocess.run(
            [os.path.join(BIN, name), *args],
            capture_output=True,
            text=True,
            env=environment,
            cwd=cwd,
            input=input_text,
            check=False,
        )

    return invoke
