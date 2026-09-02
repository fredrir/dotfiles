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
        "SYSINFO_HOST": "test",
    }
    return repo, home, env


def test_sync_help_comes_from_the_native_command(tool, sandbox):
    _repo, _home, env = sandbox
    result = tool("dotfile", "sync", "--help", env=env)
    assert result.returncode == 0
    assert "Reconcile the repository, generated metadata, and this workstation" in result.stdout
    assert "--verbose" in result.stdout


def test_sync_rejects_an_unknown_resolution_natively(tool, sandbox):
    _repo, _home, env = sandbox
    result = tool("dotfile", "sync", "--resolve", "sideways", env=env)
    assert result.returncode == 2
    assert "invalid value 'sideways'" in result.stderr


def test_private_python_backend_refuses_to_duplicate_sync(tool, sandbox):
    _repo, _home, env = sandbox
    result = tool("dotfile-py", "sync", env=env)
    assert result.returncode == 1
    assert "native dotfile executable" in result.stderr


def test_generation_commands_point_to_sync(tool, sandbox):
    _repo, _home, env = sandbox
    for command in ("docs", "packages"):
        result = tool("dotfile", command, env=env)
        assert result.returncode == 2
        assert f"'{command}' is included in 'dotfile sync'" in result.stderr


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


def test_doctor_reports_link_health(tool, sandbox):
    _repo, _home, env = sandbox
    tool("dotfile", "link", "test", env=env)
    result = tool("dotfile", "doctor", "test", env=env)
    assert result.returncode == 0
    assert "1 linked, 0 missing, 0 differing" in result.stdout


def test_doctor_fails_when_a_profile_link_is_missing(tool, sandbox):
    _repo, _home, env = sandbox
    result = tool("dotfile", "doctor", "test", env=env)
    assert result.returncode == 1
    assert "0 linked, 1 missing, 0 differing" in result.stdout


def test_status_is_no_longer_a_command(tool, sandbox):
    _repo, _home, env = sandbox
    result = tool("dotfile", "status", env=dict(env, PATH=""))
    assert result.returncode == 2
    assert "'status' is included in 'dotfile doctor'; run that instead." in result.stderr


def test_an_unregistered_name_runs_the_binary_behind_it(tool, tmp_path):
    stub = tmp_path / "dotfile-nonesuch"
    stub.write_text('#!/bin/sh\nprintf "stub %s" "$*"\nexit 7\n')
    stub.chmod(0o755)
    result = tool(
        "dotfile",
        "nonesuch",
        "--check",
        "x",
        env={"PATH": f"{tmp_path}{os.pathsep}{os.environ['PATH']}"},
    )
    # execvp, so the arguments and the exit status are the tool's own.
    assert result.stdout == "stub --check x"
    assert result.returncode == 7


def test_a_name_with_no_binary_behind_it_is_still_an_error(tool, tmp_path):
    result = tool("dotfile", "nonesuch", env={"PATH": str(tmp_path), "COLUMNS": "200"})
    assert result.returncode == 2
    assert "No such command 'nonesuch'." in result.stderr
    assert "setup.sh" in result.stderr


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
