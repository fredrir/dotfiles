import json
import subprocess
import tempfile
from pathlib import Path

import pytest


@pytest.mark.parametrize(
    "args", [("--help",), ("plugins", "--help"), ("--completions", "zsh"), ("--command-dump",)]
)
def test_help_and_completion_work_without_config_or_tmux(invoke, tmp_path, args):
    result = invoke(
        *args,
        env={
            "DOTFILES_TMUX_CONFIG": str(tmp_path / "absent"),
            "PATH": "/nonexistent",
            "TMUX_BINARY": "/nonexistent/tmux",
        },
    )
    assert result.stdout
    if args == ("--help",):
        assert "_pick" not in result.stdout
        assert "client-update" not in result.stdout


@pytest.mark.parametrize(
    "target", ["-oProxyCommand=touch /tmp/no", "archie;touch", "$(touch /tmp/no)"]
)
def test_host_rejects_shell_and_option_injection(invoke, target):
    result = invoke("host", "--", target, check=False)
    assert result.returncode != 0
    assert "invalid SSH host" in result.stderr


def test_arguments_belong_to_their_commands(invoke):
    result = invoke("scratch", "--cwd-file", "/tmp/no", check=False)
    assert result.returncode == 2
    assert "unexpected argument" in result.stderr


def test_project_creation_is_usable_outside_the_checkout(invoke, environment, tmp_path):
    project = tmp_path / "project ; $(touch SHOULD_NOT_EXIST)"
    project.mkdir()
    with tempfile.TemporaryDirectory(prefix="tw-cli-", dir="/tmp") as directory:
        socket = Path(directory) / "socket"
        try:
            result = invoke("--socket", socket, "enter", project, "--detach", cwd=tmp_path)
            again = invoke("--socket", socket, "enter", project, "--detach", cwd=tmp_path)
            assert result.stdout == again.stdout
            assert result.stdout.startswith("$")
            rows = json.loads(invoke("--socket", socket, "projects", "--json").stdout)
            assert any(row["kind"] == "session" and str(project) in row["label"] for row in rows)
            assert not (tmp_path / "SHOULD_NOT_EXIST").exists()
        finally:
            subprocess.run(
                [environment["TMUX_BINARY"], "-S", str(socket), "kill-server"],
                env=environment,
                capture_output=True,
                timeout=10,
                check=False,
            )


def test_config_is_resolved_from_xdg(environment):
    env = dict(environment)
    env.pop("DOTFILES_TMUX_CONFIG")
    result = subprocess.run(
        [environment["DOTFILES_TMUX_BINARY"], "doctor", "--json"],
        env=env,
        capture_output=True,
        text=True,
        timeout=10,
        check=True,
    )
    assert json.loads(result.stdout)["config"] == str(Path(environment["XDG_CONFIG_HOME"]) / "tmux")


def test_old_tmux_is_rejected_before_session_creation(invoke, tmp_path):
    binary = tmp_path / "old-tmux"
    binary.write_text("#!/bin/sh\ncase \"$*\" in *-V*) echo 'tmux 3.7b';; *) exit 1;; esac\n")
    binary.chmod(0o700)
    result = invoke("enter", "--detach", env={"TMUX_BINARY": str(binary)}, check=False)
    assert result.returncode == 1
    assert "3.7c or newer required" in result.stderr
