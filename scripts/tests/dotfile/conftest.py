import subprocess

import pytest


def run_git(cwd, *args):
    subprocess.run(["git", "-C", str(cwd), *args], check=True, capture_output=True)


@pytest.fixture
def vault(tool, tmp_path):
    root = tmp_path / "repo"
    home = tmp_path / "home"
    root.mkdir()
    (home / ".config" / "dotfile").mkdir(parents=True)
    (home / ".ssh").mkdir()
    (root / "environment" / "test").mkdir(parents=True)
    (root / "environment" / "test" / "manifest").write_text("shared\n")
    (root / "targets").write_text("")
    (root / "shared").mkdir()
    run_git(tmp_path, "init", "-q", str(root))
    run_git(root, "config", "user.email", "test@example.com")
    run_git(root, "config", "user.name", "test")
    (home / ".config" / "dotfile" / "profile").write_text("test\n")

    env = {
        "DOTFILE_ROOT": str(root),
        "HOME": str(home),
        "XDG_CONFIG_HOME": str(home / ".config"),
    }

    def secret(*args, editor=None):
        merged = dict(env, EDITOR=editor) if editor else env
        return tool("dotfile", "secret", *args, env=merged)

    assert secret("init").returncode == 0
    assert secret("enroll", "box").returncode == 0
    return root, home, env, secret


@pytest.fixture
def writer(tmp_path):
    made = []

    def build(text):
        script = tmp_path / f"editor{len(made)}.sh"
        script.write_text(f"#!/usr/bin/env bash\ncat > \"$1\" <<'BODY'\n{text}BODY\n")
        script.chmod(0o755)
        made.append(script)
        return str(script)

    return build
