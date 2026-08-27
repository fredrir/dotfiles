import os

import pytest


@pytest.fixture
def sandbox(tmp_path):
    repo = tmp_path / "repo"
    home = tmp_path / "home"
    (home / ".config").mkdir(parents=True)
    (repo / "shared" / "alpha").mkdir(parents=True)
    (repo / "shared" / "alpha" / "alpha.conf").write_text("alpha\n")
    (repo / "environment" / "test").mkdir(parents=True)
    (repo / "environment" / "test" / "manifest").write_text("shared\n")
    (repo / "config").mkdir()
    (repo / "config" / "targets.dotfile").write_text("")
    env = {
        "DOTFILE_ROOT": str(repo),
        "HOME": str(home),
        "XDG_CONFIG_HOME": str(home / ".config"),
    }
    return repo, home, env


def test_sync_runs_setup_with_the_sync_flag(tool, sandbox):
    repo, _home, env = sandbox
    script = repo / "setup.sh"
    script.write_text('#!/bin/sh\nprintf %s "$1" > "$(dirname "$0")/called.txt"\nexit 7\n')
    script.chmod(0o755)
    result = tool("dotfile", "sync", env=env)
    assert result.returncode == 7
    assert (repo / "called.txt").read_text() == "--sync"


def test_sync_forwards_the_linker_flags_after_a_separator(tool, sandbox):
    repo, _home, env = sandbox
    script = repo / "setup.sh"
    script.write_text('#!/bin/sh\nprintf "%s" "$*" > "$(dirname "$0")/called.txt"\n')
    script.chmod(0o755)
    result = tool(
        "dotfile", "sync", "test", "--resolve", "live", "--override", "shared=none", "-n", env=env
    )
    assert result.returncode == 0
    assert (repo / "called.txt").read_text() == (
        "--sync test -- --override shared=none -n --resolve live"
    )


def test_sync_rejects_an_unknown_resolution(tool, sandbox):
    repo, _home, env = sandbox
    script = repo / "setup.sh"
    script.write_text("#!/bin/sh\nexit 0\n")
    script.chmod(0o755)
    result = tool("dotfile", "sync", "--resolve", "sideways", env=env)
    assert result.returncode == 1
    assert "--resolve must be one of" in result.stderr


def test_sync_without_a_setup_script_fails(tool, sandbox):
    _repo, _home, env = sandbox
    result = tool("dotfile", "sync", env=env)
    assert result.returncode == 1
    assert "setup.sh" in result.stderr


def test_link_folds_a_package(tool, sandbox):
    repo, home, env = sandbox
    result = tool("dotfile", "link", "test", env=env)
    assert result.returncode == 0
    link = home / ".config" / "alpha"
    assert os.readlink(link) == str(repo / "shared" / "alpha")


def test_link_reports_conflicts_and_fails(tool, sandbox):
    _repo, home, env = sandbox
    (home / ".config" / "alpha").write_text("mine\n")
    result = tool("dotfile", "link", "test", env=env)
    assert result.returncode == 1
    assert "conflicts" in result.stdout
    assert (home / ".config" / "alpha").read_text() == "mine\n"


def test_dry_run_changes_nothing(tool, sandbox):
    _repo, home, env = sandbox
    result = tool("dotfile", "link", "test", "-n", env=env)
    assert result.returncode == 0
    assert "would:" in result.stdout
    assert not (home / ".config" / "alpha").exists()


def test_status_after_link(tool, sandbox):
    _repo, _home, env = sandbox
    tool("dotfile", "link", "test", env=env)
    result = tool("dotfile", "status", "test", env=env)
    assert result.returncode == 0
    assert "1 linked, 0 missing, 0 differing" in result.stdout


def test_format_stdin_formats_hypr_syntax(tool):
    result = tool(
        "dotfile",
        "format",
        "--stdin",
        "hyprland.conf",
        input_text="general {\nkey=value\n}\n",
    )
    assert result.returncode == 0
    assert result.stdout == "general {\n    key = value\n}\n"


def test_profiles_lists_every_manifest(tool, sandbox):
    _repo, _home, env = sandbox
    result = tool("dotfile", "profiles", env=env)
    assert result.returncode == 0
    assert result.stdout == "test\n"


def test_no_arguments_show_help(tool, sandbox):
    _repo, _home, env = sandbox
    result = tool("dotfile", env=env)
    assert result.returncode == 0
    assert "Usage" in result.stdout
