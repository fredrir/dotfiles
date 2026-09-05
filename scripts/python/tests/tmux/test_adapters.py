import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

from .harness import ROOT, SOURCE, wait_for


@pytest.mark.parametrize("system", ["mac", "linux"])
def test_wezterm_routing(system, environment):
    assert shutil.which("lua"), "lua required"
    subprocess.run(
        ["lua", "shared/wezterm/tests/tmux-workspace.lua", system],
        cwd=ROOT,
        env=environment,
        check=True,
        timeout=15,
    )


def test_zsh_integration(environment):
    assert shutil.which("zsh"), "zsh required"
    subprocess.run(
        ["zsh", "-dfi", "shared/zsh/tests/tmux.zsh"],
        cwd=ROOT,
        env=environment,
        check=True,
        timeout=15,
    )


@pytest.mark.parametrize(
    "name, args", [("tmux-workspace", ["--version"]), ("tmux-plugins", ["status", "--json"])]
)
def test_config_shims_forward_to_native_binary(name, args, environment):
    result = subprocess.run(
        [SOURCE / "bin" / name, *args],
        env=environment,
        capture_output=True,
        text=True,
        timeout=10,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    assert (
        "tmux-workspace" in result.stdout
        if name == "tmux-workspace"
        else "resurrect" in json.loads(result.stdout)
    )


def test_missing_native_binary_fails_without_recursion(environment, tmp_path):
    environment["DOTFILES_TMUX_BINARY"] = str(tmp_path / "missing")
    result = subprocess.run(
        [SOURCE / "bin/tmux-workspace", "--version"],
        env=environment,
        capture_output=True,
        text=True,
        timeout=3,
        check=False,
    )
    assert result.returncode == 127
    assert "native binary missing" in result.stderr


def stub(server, name, body):
    path = Path(server.env["PATH"].split(os.pathsep)[0]) / name
    path.write_text(f"#!{sys.executable}\n{body}\n")
    path.chmod(0o700)


def test_yazi_chooser_directory_takes_precedence(server, tmp_path):
    server.attach()
    selected = tmp_path / "selected directory"
    selected.mkdir()
    result = tmp_path / "cwd"
    stub(
        server,
        "yazi",
        f"import sys\nfrom pathlib import Path\nPath(sys.argv[sys.argv.index('--cwd-file')+1]).write_text({server.env['HOME']!r})\nPath(sys.argv[sys.argv.index('--chooser-file')+1]).write_text({str(selected)!r} + '\\n')",
    )
    server.run("yazi", "--cwd-file", result)
    assert result.read_text() == str(selected)


@pytest.mark.parametrize("phase", ["moved", "commit-uncertain", "source-stopped", "queued"])
def test_agent_follow_rechecks_uncertain_destination(server, tmp_path, phase):
    log = tmp_path / "follow.json"
    stub(
        server,
        "agent-hop",
        f"import json,sys\nfrom pathlib import Path\nif sys.argv[1] == 'status':\n print(json.dumps({{'phase': {phase!r}}}))\nelse:\n Path({str(log)!r}).write_text(json.dumps(sys.argv[1:]))",
    )
    server.run("agent-follow")
    if phase == "queued":
        assert not log.exists()
        assert "agent-remote" not in server.tm("list-windows", "-F", "#{window_name}")
    else:
        wait_for(log.exists)
        assert json.loads(log.read_text()) == ["follow", "--pane", server.pane]
